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
//! 2. `harness_runtime::desktop::build` assembles a real [`DesktopRuntime`] from the
//!    resolved [`DesktopConfig`], wiring the orchestrator's
//!    [`client::adapter::AdapterOutputStream`] to that event sink and binding the
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

use crate::HostFileSystem;
use audio_provider::{
    media_cloud::AudioProviderCloudRegistry, media_host::AppMediaHost,
    media_native::CallbackMediaDevicePorts, media_store::BoundedMediaResourceStore,
    AppMediaConversationAttachment, AudioProviderHost,
};
use bridge::lockfile::{IdeLockfile, LockfileGuard};
use bridge::McpEndpoint;
use harness_runtime::desktop::{DesktopAudio, DesktopConfig, DesktopRuntime};
use lingxi_core::host::{
    session_context::ConversationMode, CredentialStoragePolicy, OrchestratorHandle, OutputStream,
    SlashCommandDispatcher,
};

use crate::audio_bridge::{new_audio_bridge, AudioBridge};
use crate::driver::OrchestratorTurnDriver;
use crate::mcp_bridge::McpPaths;
use crate::media_bridge::{new_media_device_callback, DesktopRemoteMediaConfigProvider};
use crate::media_service_rpc::MediaServiceRpc;
use crate::router::{EngineCommandRouter, SessionStoreContext};
use crate::server::{BridgeConnection, TurnDriver};
use crate::settings_bridge::{
    active_settings_baseline, empty_settings_source, settings_error_message, SettingsContext,
    SettingsPaths,
};
use media_api::MediaOwner;

/// Keep a boot-created attachment host-owned if later assembly fails.
/// Dropping the guard closes admission synchronously and performs the drain on
/// the runtime; an unconfirmed drain remains in the process registry.
struct BootMediaAttachmentGuard {
    attachment: Option<media_runtime::MediaAttachmentHandle>,
    cloud: Arc<AudioProviderCloudRegistry>,
}

impl BootMediaAttachmentGuard {
    fn new(
        attachment: media_runtime::MediaAttachmentHandle,
        cloud: Arc<AudioProviderCloudRegistry>,
    ) -> Self {
        Self {
            attachment: Some(attachment),
            cloud,
        }
    }

    fn disarm(&mut self) {
        self.attachment = None;
    }
}

impl Drop for BootMediaAttachmentGuard {
    fn drop(&mut self) {
        let Some(attachment) = self.attachment.take() else {
            return;
        };
        attachment.begin_revoke();
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let cloud = self.cloud.clone();
        runtime.spawn(async move {
            use media_runtime::{MediaAttachmentPhase, MediaAttachmentRevocation};
            let receipt = attachment.revoke(std::time::Duration::from_secs(1)).await;
            let confirmed = match receipt {
                MediaAttachmentRevocation::Confirmed => true,
                MediaAttachmentRevocation::Unknown { .. }
                    if attachment.phase() == MediaAttachmentPhase::Revoking =>
                {
                    attachment.wait_for_drain_completion().await == MediaAttachmentPhase::Revoked
                }
                MediaAttachmentRevocation::Unknown { .. } => false,
            };
            if confirmed {
                cloud.unregister_attachment(attachment.id());
            }
        });
    }
}

#[cfg(all(test, unix))]
#[path = "boot/workspace_resume_tests.rs"]
mod workspace_resume_tests;

#[path = "catalog_worker.rs"]
mod catalog_worker;
#[path = "history_page.rs"]
mod history_page;

/// Serve bounded metadata requests without resolving providers or building an engine.
pub async fn run_session_catalog_worker() -> Result<(), String> {
    catalog_worker::run(tokio::io::stdin(), tokio::io::stdout()).await
}

/// Env override for the API base URL (mirrors `apps/cli`'s `resolve_api_base`).
pub const API_BASE_ENV: &str = "LINGXI_API_BASE_URL";
/// The canonical Anthropic endpoint used when [`API_BASE_ENV`] is unset.
pub const DEFAULT_API_BASE: &str = "https://api.anthropic.com";
/// Maximum size of an injected plugin secret.
pub const MAX_PLUGIN_SECRET_BYTES: usize = 16 * 1024;
/// Maximum accepted JSON credential envelope size from `--credential-stdin`.
pub const MAX_STDIN_CREDENTIAL_BYTES: usize = 2 * 1024 * 1024;

/// Parsed command-line flags the binary accepts (a tiny hand-rolled parser;
/// `apps/bridge-server` deliberately avoids a clap dependency).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BridgeArgs {
    /// Actual Electron application version for the public model catalog.
    pub app_version: Option<String>,
    /// `--cwd <dir>`: working directory to root the engine at. The process
    /// `chdir`s into this before resolving the rest of the config so the
    /// settings / hook / `.mcp.json` loaders read the same dir.
    pub cwd: Option<PathBuf>,
    /// `--session-id <uuid>`: stable session identity supplied by the Desktop host.
    pub session_id: Option<String>,
    /// Exact UUID spelling retained for Native catalog path lookup.
    pub catalog_session_id: Option<String>,
    /// Exact persisted source selected by the host before lifecycle hooks run.
    pub session_transcript_path: Option<PathBuf>,
    /// Restore an owned, unsent desktop draft from its canonical metadata source.
    pub restore_draft: bool,
    /// Retain a newly created owned draft under its explicit display title.
    pub durable_draft_title: Option<String>,
    /// Device default for an uninitialized draft, applied before startup hooks.
    pub initial_conversation_mode: Option<ConversationMode>,
    /// Required ownership for an explicitly selected persisted transcript.
    pub expected_conversation_mode: Option<ConversationMode>,
    /// `--list-sessions-json`: read the session catalog and exit without building an engine.
    pub list_sessions_json: bool,
    /// Read the merged workspace environment without building an engine.
    pub workspace_environment_json: bool,
    /// Resolve product-owned directories without loading workspace settings.
    pub product_paths_json: bool,
    /// Execute one private Host account metadata command without a project engine.
    pub provider_account_command: bool,
    /// Private catalog scope; accepted only beside `--list-sessions-json`.
    pub session_scope: Option<session::jsonl::SessionDiscoveryScope>,
    /// Explicit session grants replayed by the Desktop during a workspace remount.
    pub add_dir: Vec<PathBuf>,
    /// Original session hook root retained across a workspace remount.
    pub session_project_root: Option<PathBuf>,
    /// Persistent configuration owners captured before project env is overlaid.
    pub session_config_home: Option<PathBuf>,
    pub session_global_config_path: Option<PathBuf>,
    /// Session prompt roots replayed without creating permission grants.
    pub session_prompt_directory: Vec<PathBuf>,
    /// `--model <id>`: the default model id (overrides the desktop default).
    pub model: Option<String>,
    /// Host policy for permission prompts (1..=86400 seconds).
    pub permission_timeout_secs: Option<u64>,
    /// Host policy for questions: never, 60s, 5m, or 10m.
    pub ask_user_question_timeout: Option<tool_ui::ask_user_question::AskUserQuestionTimeout>,
    /// Read the bounded account credential envelope from stdin.
    pub credential_stdin: bool,
    /// Allow workspace-controlled configuration and customization sources.
    pub trusted_workspace: bool,
    /// Private parent-provided local policy verdict for `/insights`.
    pub insights_policy_gate: Option<bool>,
    /// Private parent-provided local policy verdict for `/team-onboarding`.
    pub team_onboarding_policy_gate: Option<bool>,
    /// Own versioned scheduled tasks for this scope independently of chat runtimes.
    pub scheduled_controller: bool,
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
                "--app-version" => {
                    out.app_version = Some(
                        it.next()
                            .ok_or("--app-version requires a version")?
                            .as_ref()
                            .to_owned(),
                    );
                }
                "--list-sessions-json" => out.list_sessions_json = true,
                "--workspace-environment-json" => out.workspace_environment_json = true,
                "--product-paths-json" => out.product_paths_json = true,
                "--provider-account-command" => out.provider_account_command = true,
                "--session-scope" => {
                    let value = it
                        .next()
                        .ok_or("--session-scope requires current, repository, or all")?;
                    out.session_scope = Some(match value.as_ref() {
                        "current" => session::jsonl::SessionDiscoveryScope::Current,
                        "repository" => session::jsonl::SessionDiscoveryScope::Repository,
                        "all" => session::jsonl::SessionDiscoveryScope::All,
                        _ => {
                            return Err(
                                "--session-scope requires current, repository, or all".into()
                            )
                        }
                    });
                }
                "--add-dir" => {
                    let directory = it.next().ok_or("--add-dir requires a directory argument")?;
                    out.add_dir.push(PathBuf::from(directory.as_ref()));
                }
                "--session-project-root" => {
                    let directory = it
                        .next()
                        .ok_or("--session-project-root requires a directory argument")?;
                    let path = PathBuf::from(directory.as_ref());
                    if !path.is_absolute() {
                        return Err("--session-project-root must be an absolute path".into());
                    }
                    out.session_project_root = Some(path);
                }
                "--session-config-home" | "--session-global-config-path" => {
                    let flag = arg.as_ref();
                    let value = it
                        .next()
                        .ok_or_else(|| format!("{flag} requires an absolute path"))?;
                    let path = PathBuf::from(value.as_ref());
                    if !path.is_absolute() {
                        return Err(format!("{flag} requires an absolute path"));
                    }
                    if flag == "--session-config-home" {
                        out.session_config_home = Some(path);
                    } else {
                        out.session_global_config_path = Some(path);
                    }
                }
                "--session-prompt-directory" => {
                    let value = it
                        .next()
                        .ok_or("--session-prompt-directory requires an absolute path")?;
                    let path = PathBuf::from(value.as_ref());
                    if !path.is_absolute() {
                        return Err("--session-prompt-directory requires an absolute path".into());
                    }
                    out.session_prompt_directory.push(path);
                }
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
                    let parsed = lingxi_core::types::SessionId::parse_prefixed(value.as_ref())
                        .ok_or_else(|| "--session-id must be a valid UUID".to_string())?;
                    out.session_id = Some(parsed.as_uuid().to_string());
                    out.catalog_session_id =
                        Some(if uuid::Uuid::parse_str(value.as_ref()).is_ok() {
                            value.as_ref().to_owned()
                        } else {
                            parsed.as_uuid().to_string()
                        });
                }
                "--session-transcript-path" => {
                    let value = it
                        .next()
                        .ok_or("--session-transcript-path requires an absolute JSONL path")?;
                    let path = PathBuf::from(value.as_ref());
                    if !path.is_absolute() {
                        return Err(
                            "--session-transcript-path requires an absolute JSONL path".into()
                        );
                    }
                    out.session_transcript_path = Some(path);
                }
                "--restore-draft" => out.restore_draft = true,
                "--durable-draft-title" => {
                    let title = it.next().ok_or("--durable-draft-title requires a title")?;
                    let title = title.as_ref();
                    if title.trim().is_empty() || title.contains('\0') {
                        return Err("--durable-draft-title requires a nonempty title".into());
                    }
                    out.durable_draft_title = Some(title.to_owned());
                }
                "--initial-conversation-mode" => {
                    let value = it
                        .next()
                        .ok_or("--initial-conversation-mode requires chat or code")?;
                    out.initial_conversation_mode = Some(
                        ConversationMode::from_str(value.as_ref())
                            .ok_or("--initial-conversation-mode requires chat or code")?,
                    );
                }
                "--expected-conversation-mode" => {
                    let value = it
                        .next()
                        .ok_or("--expected-conversation-mode requires chat or code")?;
                    out.expected_conversation_mode = Some(
                        ConversationMode::from_str(value.as_ref())
                            .ok_or("--expected-conversation-mode requires chat or code")?,
                    );
                }
                "--model" => {
                    let v = it
                        .next()
                        .ok_or_else(|| "--model requires a model id argument".to_string())?;
                    out.model = Some(v.as_ref().to_string());
                }
                "--permission-timeout-secs" => {
                    let value = it
                        .next()
                        .ok_or("--permission-timeout-secs requires seconds")?;
                    let seconds: u64 = value.as_ref().parse().map_err(|_| {
                        "--permission-timeout-secs requires an integer from 1 to 86400"
                    })?;
                    if !(1..=86400).contains(&seconds) {
                        return Err("--permission-timeout-secs must be from 1 to 86400".into());
                    }
                    out.permission_timeout_secs = Some(seconds);
                }
                "--ask-user-question-timeout" => {
                    let value = it
                        .next()
                        .ok_or("--ask-user-question-timeout requires never, 60s, 5m, or 10m")?;
                    out.ask_user_question_timeout = Some(
                        tool_ui::ask_user_question::AskUserQuestionTimeout::from_settings_str(
                            value.as_ref(),
                        )
                        .ok_or("--ask-user-question-timeout requires never, 60s, 5m, or 10m")?,
                    );
                }
                "--credential-stdin" => out.credential_stdin = true,
                "--trusted-workspace" => out.trusted_workspace = true,
                "--host-insights-policy-gate" => {
                    let value = it.next().ok_or_else(|| {
                        "--host-insights-policy-gate requires true or false".to_string()
                    })?;
                    out.insights_policy_gate = Some(match value.as_ref() {
                        "true" => true,
                        "false" => false,
                        _ => {
                            return Err(
                                "--host-insights-policy-gate requires true or false".to_string()
                            )
                        }
                    });
                }
                "--host-team-onboarding-policy-gate" => {
                    let value = it.next().ok_or_else(|| {
                        "--host-team-onboarding-policy-gate requires true or false".to_string()
                    })?;
                    out.team_onboarding_policy_gate = Some(match value.as_ref() {
                        "true" => true,
                        "false" => false,
                        _ => {
                            return Err("--host-team-onboarding-policy-gate requires true or false"
                                .to_string())
                        }
                    });
                }
                "--scheduled-controller" => out.scheduled_controller = true,
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
        if out.session_scope.is_some() && !out.list_sessions_json {
            return Err("--session-scope requires --list-sessions-json".into());
        }
        if out.restore_draft
            && (out.session_id.is_none()
                || out.session_transcript_path.is_some()
                || out.list_sessions_json
                || out.workspace_environment_json
                || out.product_paths_json)
        {
            return Err(
                "--restore-draft requires a runtime session UUID and no selected transcript".into(),
            );
        }
        if out.initial_conversation_mode.is_some() && !out.restore_draft {
            return Err("--initial-conversation-mode requires --restore-draft".into());
        }
        if out.restore_draft && out.initial_conversation_mode.is_none() {
            return Err("--restore-draft requires --initial-conversation-mode".into());
        }
        if let (Some(initial), Some(expected)) = (
            out.initial_conversation_mode,
            out.expected_conversation_mode,
        ) {
            if initial != expected {
                return Err("initial and expected conversation modes must match".into());
            }
        }
        if out.durable_draft_title.is_some()
            && (!out.restore_draft || out.expected_conversation_mode.is_some())
        {
            return Err("--durable-draft-title requires a new owned draft".into());
        }
        if out.expected_conversation_mode.is_some() && out.session_id.is_none() {
            return Err("--expected-conversation-mode requires --session-id".into());
        }
        if let Some(path) = &out.session_transcript_path {
            let id = out
                .session_id
                .as_deref()
                .ok_or("--session-transcript-path requires --session-id")?;
            validate_selected_transcript_identity(path, id)?;
            if out.workspace_environment_json
                || out.product_paths_json
                || out.provider_account_command
            {
                return Err(
                    "--session-transcript-path requires a runtime launch or session catalog lookup"
                        .into(),
                );
            }
            if out.list_sessions_json {
                if out.session_scope.is_none() {
                    return Err("a selected catalog transcript requires --session-scope".into());
                }
                if out.expected_conversation_mode.is_some() {
                    return Err(
                        "a selected catalog transcript reports its own conversation mode".into(),
                    );
                }
            } else if out.expected_conversation_mode.is_none() {
                return Err(
                    "--session-transcript-path requires --expected-conversation-mode".into(),
                );
            }
        }
        if out.workspace_environment_json && (out.list_sessions_json || out.session_id.is_some()) {
            return Err("--workspace-environment-json accepts only a workspace directory".into());
        }
        if out.product_paths_json {
            let product_args = Self {
                product_paths_json: true,
                cwd: out.cwd.clone(),
                help: out.help,
                ..Self::default()
            };
            if out != product_args {
                return Err("--product-paths-json accepts only a workspace directory".into());
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
             --permission-timeout-secs <SECONDS>  Permission wait (1..86400)\n    \
             --ask-user-question-timeout <never|60s|5m|10m>\n    \
             --model <ID>     Default model id (default: the desktop build default)\n    \
             --credential-stdin\n    \
                              Read a provider credential envelope from stdin\n    \
             --trusted-workspace\n    \
                              Enable workspace settings, hooks, MCP, agents, plugins, and memory\n    \
             --scheduled-controller\n    \
                              Own versioned scheduled tasks for this workspace\n    \
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

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
/// One bounded credential payload accepted from the Electron parent.
pub struct CredentialEnvelope {
    /// Secret-free Host account directory for this credential domain.
    #[serde(default)]
    pub provider_accounts: Option<lingxi_core::host::provider_accounts::AccountSnapshot>,
    /// Canonical Host account root, independent of the session settings root.
    #[serde(default)]
    pub provider_account_home: Option<PathBuf>,
    /// Host credential domain, available before the deferred account projection.
    #[serde(default)]
    pub provider_account_backend: Option<String>,
    /// Private access-only account material. Never forwarded to clients.
    #[serde(default)]
    pub provider_account_credentials: BTreeMap<String, secret::AccountCredential>,
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
    crate::provider_accounts::validate_injection(
        envelope.provider_accounts.as_ref(),
        &envelope.provider_account_credentials,
    )?;
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
            if secret.is_empty() || secret.len() > MAX_PLUGIN_SECRET_BYTES || secret.contains('\0')
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

async fn desktop_provider_catalog_listings(
    cfg: &DesktopConfig,
    region: llm_runtime::Region,
) -> Vec<lingxi_core::host::ModelListing> {
    let assembled = provider_config::assemble_for_region(
        provider_config::AssembleInputs {
            official_catalog: match &cfg.startup_catalog {
                Some(source) => Some((*source.load().await).clone()),
                None => None,
            },
            anthropic_api_base: cfg.api_base.clone(),
            anthropic_models: desktop_provider_catalog_anthropic_models(
                &cfg.default_model,
                cfg.fallback_model.as_deref(),
            ),
            anthropic_has_api_key: false,
            anthropic_has_oauth: false,
            user_providers: cfg.provider_profiles.clone().unwrap_or_default(),
            routing: cfg.routing.clone(),
        },
        region,
    );
    for warning in &assembled.warnings {
        tracing::warn!(warning = %warning, "bridge-server provider catalog assembly");
    }
    llm_runtime::ModelRegistry::from_config(assembled.client_config)
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
) -> Vec<llm_runtime::ModelProfile> {
    let caps = llm_runtime::Capabilities {
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
        let (profile, bare) = llm_runtime::split_profile_model(value);
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
        .map(|id| llm_runtime::ModelProfile {
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

/// Read provider profiles and routing from the Host settings file. Project and
/// local settings cannot replace these fields; invalid Host settings fail boot.
#[must_use]
fn load_settings_blocks(
    home: &std::path::Path,
) -> Result<
    (
        Option<BTreeMap<String, serde_json::Value>>,
        Option<serde_json::Value>,
    ),
    String,
> {
    match lingxi_core::settings::loader::read_settings_file(&home.join("settings.json")) {
        Ok(Some(settings)) => Ok((settings.providers, settings.routing)),
        Ok(None) => Ok((None, None)),
        Err(_) => Err("Host provider settings are unavailable".into()),
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

/// Brand data home: the branding crate's configuration-directory override when
/// set to a non-empty value, otherwise the branded hidden home directory.
/// Private configuration and session state; workspace files use a sibling root.
#[must_use]
pub fn brand_home() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(branding::CONFIG_DIR_ENV).filter(|dir| !dir.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    dirs::home_dir().map(|h| h.join(branding::DOT_DIR))
}

/// Resolve a deterministic [`DesktopConfig`] from [`BridgeArgs`] + `std::env`.
///
/// Mirrors `apps/cli/src/init.rs`'s `resolve_desktop_config` field-for-field,
/// with the one production difference for a transport: `use_noop_permission_gate`
/// is `false`, so `harness_runtime::desktop::build` binds the connection-scoped
/// `AdapterPermissionGate` (the WS permission round-trip, F2-06).
///
/// `cwd` is `std::env::current_dir()` — the caller is expected to have already
/// `chdir`'d into any `--cwd` (so the settings / hook / `.mcp.json` loaders read
/// the same dir).
///
/// Provider authentication is supplied through the account directory and exact
/// account credential records during assembly.
#[must_use]
pub fn resolve_desktop_config(args: &BridgeArgs) -> Result<DesktopConfig, String> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let lingxi_home = args
        .session_config_home
        .clone()
        .or_else(brand_home)
        .unwrap_or_else(|| PathBuf::from("/dev/null"));
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

    let (profiles, routing) = load_settings_blocks(&lingxi_home)?;
    let provider_profiles = without_settings_credentials(profiles);
    let trusted = args.trusted_workspace;

    let build_info = harness_runtime::desktop::BuildInfo::new(
        env!("CARGO_PKG_VERSION"),
        option_env!("LINGXI_GIT_SHA_SHORT").unwrap_or("unknown"),
    );
    let api_base = resolve_api_base();
    let anthropic_client_metadata = host_identity::native_anthropic_client_metadata_for_host(
        &api_base,
        "lingxi-desktop",
        build_info.version,
    );

    {
        let mut __desktop_config = DesktopConfig::default();
        __desktop_config.build_info = build_info;
        __desktop_config.diagnostics = None;
        __desktop_config.permission_timeout = args
            .permission_timeout_secs
            .map(std::time::Duration::from_secs);
        __desktop_config.ask_user_question_timeout_override = args.ask_user_question_timeout;
        __desktop_config.composition = None;
        __desktop_config.defer_session_start = false;
        __desktop_config.initial_conversation_mode = args
            .expected_conversation_mode
            .or(args.initial_conversation_mode);
        // The desktop opens a session while its startup MCP servers connect;
        // `spawn_mcp_startup_publisher` re-publishes the listings as each
        // settles.
        __desktop_config.connect_mcp_in_background = true;
        __desktop_config.session_transcript_path = args.session_transcript_path.clone();
        __desktop_config.session_transcript_authority = None;
        __desktop_config.restore_draft = args.restore_draft;
        __desktop_config.durable_draft_title = args.durable_draft_title.clone();
        __desktop_config.session_resume_snapshot = None;
        __desktop_config.session_resume_cost = None;
        __desktop_config.max_structured_output_retries = 5;
        __desktop_config.mcp_services_factory = None;
        __desktop_config.user_agent_environment = None;
        __desktop_config.request_identity = None;
        __desktop_config.anthropic_compatible_version = None;
        __desktop_config.native_thinking_display = None;
        __desktop_config.verified_computer_profiles = Vec::new();
        __desktop_config.host_workspace_trusted = Some(trusted);
        __desktop_config.insights_policy_gate = args.insights_policy_gate;
        __desktop_config.team_onboarding_policy_gate = args.team_onboarding_policy_gate;
        __desktop_config.mod_render_surface =
            Some(harness_runtime::desktop::ModRenderSurface::Desktop);
        __desktop_config.native_settings_agent_context =
            lingxi_core::host::NativeSettingsAgentContext::Main;
        __desktop_config.enable_automation_scheduler = args.scheduled_controller;
        __desktop_config.initial_teammate_team_name = None;
        __desktop_config.api_base = api_base;
        __desktop_config.anthropic_client_metadata = anthropic_client_metadata;
        // Authentication belongs to the account directory; config carries no key.
        // The packaged bridge gets an explicit process-local store. Desktop,
        // CLI, and TUI own persistent broker access; this sidecar receives
        // access-only account records over stdin.
        __desktop_config.isolated_credential_storage = false;
        __desktop_config.credential_storage_policy = if args.packaged_credential_stdin_only {
            CredentialStoragePolicy::NativeOrMemory
        } else {
            CredentialStoragePolicy::NativePreferred
        };
        __desktop_config.injected_plugin_secrets = BTreeMap::new();
        __desktop_config.initial_project_root = args.session_project_root.clone();
        __desktop_config.initial_prompt_directories = args.session_prompt_directory.clone();
        __desktop_config.cwd = cwd;
        // Bridge-server is a local host; it exposes no remote workspace,
        // diskless session, or pre-resolved Windows network-drive roots.
        __desktop_config.remote_workspace = false;
        __desktop_config.diskless = false;
        __desktop_config.trusted_network_directories = Vec::new();
        __desktop_config.lingxi_home = lingxi_home;
        __desktop_config.startup_catalog = args.app_version.as_ref().map(|version| {
            harness_runtime::StartupCatalog::new(
                harness_runtime::CatalogPlatform::Desktop,
                version,
                &__desktop_config.lingxi_home,
            )
        });
        __desktop_config.default_model = default_model;
        // Only a `--model` arg is an explicit per-session choice; the built-in
        // default stays eligible for the engine's connected-provider fallback.
        __desktop_config.default_model_explicit = args.model.is_some();
        // The bridge does not read the TUI's `settings.recentModels`; the
        // fallback uses the static provider order only.
        __desktop_config.recent_models = Vec::new();
        // The Electron bridge does not expose --fallback-model (CLI --print
        // only); the Opus consecutive-529 fallback stays disabled here.
        __desktop_config.fallback_model = None;
        // The Electron host currently exposes no custom beta-header flag.
        __desktop_config.custom_betas = Vec::new();
        // The Electron host does not expose CLI `--settings` / flagSettings.
        __desktop_config.flag_settings = None;
        __desktop_config.flag_settings_source = None;
        __desktop_config.provider_profiles = provider_profiles;
        __desktop_config.routing = routing;
        __desktop_config.mcp_paths = if trusted {
            vec![project_mcp_path, global_mcp_path]
        } else {
            Vec::new()
        };
        // A transport binds the connection-scoped AdapterPermissionGate (F2-06).
        __desktop_config.use_noop_permission_gate = false;
        // Transport host: the adapter gate IS the enforcement, so headless
        // deny-on-ask does not apply (only consulted when use_noop is true).
        __desktop_config.deny_unresolved_ask = false;
        // Transport host: no process stdout TTY for `tengu_api_success.isTTY`.
        __desktop_config.is_tty = false;
        // Transport host: no injected interactive gate (that is the TUI's path).
        __desktop_config.injected_permission_gate = None;
        // The headless bridge has no interactive /fork or /resume-as-background
        // surface, so it wires no background-session forker seam.
        __desktop_config.bg_session_forker = None;
        // Placeholder — `assemble_with_account_credentials` installs the
        // connection-scoped AskUserQuestion broker once the event sink exists.
        __desktop_config.ask_user_question_tx = None;
        // Placeholder — `resolve_desktop_config` has no live connection to
        // build a sink from yet. `assemble_with_account_credentials` overwrites this
        // to `Some(sender)` once the `BridgeConnection` (and therefore its
        // `computer_access_sink()`) exists, wiring the Electron-facing
        // `ComputerAccessBroker`.
        __desktop_config.computer_access_tx = None;
        // Filled by `assemble_with_account_credentials` after the connection-scoped
        // event sink exists; this keeps subagent lifecycle events on the same
        // authenticated session stream as ordinary turn events.
        __desktop_config.session_agent_observer = None;
        // M10: the bridge-server does not start a coordinator session by
        // default (threading this from session metadata is a follow-up).
        __desktop_config.session_started_as_coordinator = false;
        // Production memory: load the real `<cwd>/LINGXI.md` +
        // `~/.lingxi/LINGXI.md` hierarchy into the system prompt (claude-code
        // parity), which also makes the session-start
        // `fire_instructions_loaded()` fire over those files. Tests inject a
        // controlled provider (or `None`); only this real-host path reads the FS.
        __desktop_config.memory_provider = trusted.then(orchestrator::prompt::real_provider);
        // Trusted desktop sessions share the engine Auto default. Workspace
        // trust is still required before granting autonomous permissions.
        __desktop_config.permission_mode = if trusted {
            permission::PermissionMode::Auto
        } else {
            permission::PermissionMode::Default
        };
        __desktop_config.permission_mode_cli = None;
        __desktop_config.permission_mode_preference = None;
        __desktop_config.permission_mode_cli_explicit = false;
        // The Electron bridge exposes no --max-turns / --max-budget flags;
        // both stay unset (unbounded), matching the CLI defaults.
        __desktop_config.max_turns = None;
        __desktop_config.plan_mode_instructions = None;
        __desktop_config.plans_directory = None;
        __desktop_config.max_budget_usd = None;
        // The bridge has no structured-output flag; unconstrained turns.
        __desktop_config.json_schema = None;
        // The bridge has no --system-prompt / --append-system-prompt CLI flags.
        __desktop_config.system_prompt_override = None;
        __desktop_config.append_system_prompt = None;
        // The Desktop host owns the session UUID so multiple bridge processes can
        // address one another without relying on generated display names.
        __desktop_config.session_id_override = args.session_id.clone();
        __desktop_config.session_writer_lease = None;
        __desktop_config.parent_session_id = None;
        // The Electron bridge has no --disable-slash-commands flag.
        __desktop_config.disable_slash_commands = false;
        // There is no per-session skill allowlist input on the bridge. `None`
        // keeps all otherwise eligible registered skills model-invocable.
        __desktop_config.session_skill_allowlist = None;
        // The Electron bridge has no --add-dir flag.
        __desktop_config.add_dir = args.add_dir.clone();
        // The Electron bridge has no --mcp-config flag.
        __desktop_config.cli_mcp_servers = Vec::new();
        // The Electron bridge has no --strict-mcp-config flag either; ambient
        // MCP configs stay eligible (trust gating above still applies) and
        // agent-frontmatter servers are never strict-skipped.
        __desktop_config.strict_mcp_config = false;
        __desktop_config.restricted = false;
        __desktop_config.restricted_tools = None;
        // The Electron bridge has no --exclude-dynamic-system-prompt-sections flag.
        __desktop_config.exclude_dynamic_system_prompt_sections = false;
        // The bridge has no `--setting-sources` flag; load all tiers.
        __desktop_config.setting_source_scope = if trusted {
            (true, true)
        } else {
            (false, false)
        };
        // The Electron bridge has no --safe-mode / --bare flags (CLI-only
        // reduced modes); all customizations load.
        __desktop_config.customization_gates = if trusted {
            harness_runtime::desktop::CustomizationGates::default()
        } else {
            harness_runtime::desktop::CustomizationGates {
                safe_mode: true,
                bare: false,
            }
        };
        // The Electron bridge has no --no-session-persistence flag; persist.
        __desktop_config.session_persistence = true;
        // (M4 cc2.1.198) The Electron bridge exposes none of --agents /
        // --agent / --plugin-dir / --effort (CLI session flags).
        __desktop_config.cli_agents_json = None;
        __desktop_config.cli_agent = None;
        __desktop_config.cli_plugin_dirs = Vec::new();
        __desktop_config.initial_effort = None;
        // The Electron bridge (accepted divergence) keeps the byte-identical
        // defaults for the ANTHROPIC_MODEL env-pin exemption + boot thinking
        // config; the CLI `resolve_desktop_config` is the parity surface.
        __desktop_config.default_model_env_pinned = false;
        __desktop_config.session_thinking = Default::default();
        // The Electron bridge has no `-w`/`--worktree` flag; inert boot (no
        // worktree launch).
        __desktop_config.worktree_launch = None;
        // The Electron bridge has no `--tmux` flag; inert (no tmux session).
        __desktop_config.tmux_launch = None;
        // The trusted Electron shell exposes an explicit, user-operated Full
        // access selector. This only makes the live bypass mode AVAILABLE; the
        // session still boots in Default and changes mode only after an
        // authenticated renderer command crosses the bridge IPC boundary.
        __desktop_config.allow_dangerously_skip_permissions = trusted;
        // Device audio is CONNECTION-scoped, not argv-scoped: the bridge it
        // proxies through cannot exist until `assemble` has a connection to
        // build it over, so it is filled there and never here.
        __desktop_config.audio = None;
        // Electron renders inline widgets (`lingxi-viz://visualization`), so
        // the `Visualization` tool and `/visualize` skill are offered here.
        __desktop_config.inline_visualization = true;
        // Electron opens a private page inside the app. The page stays on this computer.
        __desktop_config.private_pages = true;
        // Electron owns the desktop project library, application windows and process lifecycle.
        __desktop_config.local_apps = true;
        Ok(__desktop_config)
    }
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
    runtime: Arc<DesktopRuntime>,
    /// The request half of this connection's audio bridge, used by the routed
    /// audio service. Its responder half is bound to `connection`; retaining it
    /// here keeps that pairing visible at the composition root.
    audio: Arc<AudioBridge>,
    /// The single shared media facade used by Desktop tools, UI/Remote RPC,
    /// and the audio service projection.
    media_host: Arc<AppMediaHost>,
    /// Host-control transport handles retained by the product Remote media
    /// connector for this authenticated Bridge connection.
    media_connection_channel: Arc<crate::media_bridge::BridgeMediaConnectionChannel>,
    media_control_channel: Arc<crate::media_bridge::BridgeMediaControlChannel>,
    /// Keeps the cross-session live identity and inbox registered for the
    /// lifetime of this bridge process.
    #[allow(dead_code)]
    live_session: LiveSessionGuard,
}

impl BoundServer {
    #[cfg(test)]
    pub(crate) fn runtime_for_test(&self) -> &Arc<DesktopRuntime> {
        &self.runtime
    }

    /// The names of the tools the assembled engine registered.
    ///
    /// Delegates to [`DesktopRuntime::registered_tool_names`]; the runtime
    /// itself stays inside this struct.
    #[must_use]
    pub fn registered_tool_names(&self) -> Vec<String> {
        self.runtime.registered_tool_names()
    }

    /// This connection's low-level `AudioService` bridge.
    #[must_use]
    pub fn audio(&self) -> &Arc<AudioBridge> {
        &self.audio
    }

    /// Return the exact app-scoped media host injected into this runtime.
    #[must_use]
    pub fn media_host(&self) -> &Arc<AppMediaHost> {
        &self.media_host
    }

    /// Authenticated route negotiation channel for the product connector.
    #[must_use]
    pub fn media_connection_channel(
        &self,
    ) -> &Arc<crate::media_bridge::BridgeMediaConnectionChannel> {
        &self.media_connection_channel
    }

    /// Authenticated render-barrier channel for the product connector.
    #[must_use]
    pub fn media_control_channel(&self) -> &Arc<crate::media_bridge::BridgeMediaControlChannel> {
        &self.media_control_channel
    }

    /// Capture the ordered session lifecycle owner before the connection is
    /// moved into the endpoint. It retains task, cost, coordinator, and outbox
    /// authorities until host teardown has drained accepted work.
    #[must_use]
    pub fn session_lifecycle(&self) -> Arc<harness_runtime::desktop::DesktopSessionLifecycle> {
        self.runtime.session_lifecycle.clone()
    }

    /// Observe a source-scoped failure to validate a required Native policy
    /// semantic. Kept separate from the Native refusal and RequestExit paths.
    pub fn wait_for_policy_semantic_worker_failure(
        &self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = std::sync::Arc<
                        harness_runtime::desktop::NativePolicySemanticWorkerFailure,
                    >,
                > + Send,
        >,
    > {
        let mut probe = self.runtime.policy_helper_semantic_worker_failure.clone();
        Box::pin(async move { probe.wait().await })
    }

    /// Observe authenticated RequestExit or a managed-policy refusal without
    /// borrowing the moved connection. A refusal retains its typed source error.
    pub fn wait_for_exit_requested(
        &self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Option<harness_runtime::desktop::PolicyHelperStartupError>,
                > + Send,
        >,
    > {
        let orchestrator = self.runtime.orchestrator.clone();
        let refusal = self.runtime.policy_helper_refusal.clone();
        Box::pin(async move {
            loop {
                if orchestrator.current_should_exit() {
                    return refusal.current().map(|error| error.as_ref().clone());
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        })
    }
}

/// Process-local live-session registration owned by a bridge runtime.
///
/// The registry and inbox are intentionally process-wide in `traits`, just as
/// they are for the CLI. Holding this guard in `BoundServer` makes the bridge
/// lifecycle explicit and guarantees graceful cleanup without changing the
/// engine wire protocol.
struct LiveSessionGuard {
    dir: lingxi_core::host::live_sessions::LiveSessionDir,
    pid: u32,
    session_id: String,
    generation: u64,
    inbox_started: bool,
    _writer_claim: Option<lingxi_core::host::live_sessions::SharedSessionWriterLease>,
}

#[derive(Debug)]
struct LiveSessionGenerationState {
    next: u64,
    active: Option<u64>,
}

static LIVE_SESSION_GENERATIONS: std::sync::Mutex<LiveSessionGenerationState> =
    std::sync::Mutex::new(LiveSessionGenerationState {
        next: 0,
        active: None,
    });

fn install_live_session_generation() -> Result<u64, String> {
    let mut state = LIVE_SESSION_GENERATIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let generation = state
        .next
        .checked_add(1)
        .ok_or_else(|| "bridge live-session generation exhausted".to_string())?;
    state.next = generation;
    state.active = Some(generation);
    Ok(generation)
}

fn retire_live_session_generation(generation: u64) -> bool {
    let mut state = LIVE_SESSION_GENERATIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if state.active != Some(generation) {
        return false;
    }
    state.active = None;
    true
}

impl LiveSessionGuard {
    /// Drop the bootstrap's claim clone after engine composition has consumed
    /// the same construction-only Arc. The hydrated coordinator/manager now
    /// owns the authority and can release it when its live scopes drain.
    fn release_writer_claim(&mut self) {
        self._writer_claim.take();
    }
}

impl Drop for LiveSessionGuard {
    fn drop(&mut self) {
        // Process session/presence and the UDS runtime are global. A newer
        // bridge assembled in this process supersedes the old guard; only that
        // newest generation may tear the globals down. Otherwise dropping an
        // old A/B server could stop and unregister an unrelated live C server.
        if !retire_live_session_generation(self.generation) {
            return;
        }
        if self.inbox_started {
            if let Err(error) = lingxi_core::host::uds_inbox::stop_process_inbox_checked() {
                tracing::warn!(%error, "bridge inbox shutdown could not preserve accepted work");
            }
        }
        // Clear/resume may have moved this process's presence record to a
        // successfully hydrated destination.  Drop must unregister that
        // current record, not the construction-time A target, while old A
        // durable scopes continue to own their independent writer lease.
        let current = lingxi_core::host::live_sessions::process_session_id()
            .unwrap_or_else(|| self.session_id.clone());
        // `stop_process_inbox_checked` has just spilled accepted/held work into
        // `<session>.inbox`. Remove only this PID's matching presence metadata;
        // the inbox is durable work, and writer-claim ownership belongs to the
        // coordinator/last scoped owner rather than this registration guard.
        if let Err(error) = self.dir.unregister_record_if_session(self.pid, &current) {
            tracing::warn!(%error, "bridge live-session metadata cleanup failed");
        }
    }
}

fn canonical_session_id(raw: Option<&str>) -> Result<String, String> {
    raw.map(|value| {
        lingxi_core::types::SessionId::parse_prefixed(value)
            .map(|id| id.as_uuid().to_string())
            .ok_or_else(|| "bridge session id must be a valid UUID".to_string())
    })
    .unwrap_or_else(|| Ok(lingxi_core::types::SessionId::new().as_uuid().to_string()))
}

fn validate_selected_transcript_identity(path: &Path, id: &str) -> Result<uuid::Uuid, String> {
    let expected = uuid::Uuid::parse_str(id)
        .map_err(|_| "selected transcript requires a valid session UUID")?;
    let source_id = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| session::jsonl::validate_uuid(stem))
        .and_then(|stem| uuid::Uuid::parse_str(stem).ok());
    if !path.is_absolute()
        || path.to_str().is_none()
        || path.extension().and_then(|extension| extension.to_str()) != Some("jsonl")
        || source_id != Some(expected)
    {
        return Err(
            "selected transcript path must be an absolute JSONL file matching the session UUID"
                .into(),
        );
    }
    Ok(expected)
}

fn validate_selected_session_mode(
    content: &str,
    uuid: uuid::Uuid,
    expected: ConversationMode,
) -> Result<(), String> {
    let actual = session::JsonlWriter::fixed_session_mode_from_source(content, &uuid.to_string())
        .map_err(|error| format!("invalid session mode: {error}"))?;
    if actual != expected {
        return Err(format!(
            "session_mode_mismatch: session {uuid} belongs to {}",
            actual.as_str()
        ));
    }
    Ok(())
}

/// Capture a catalog-selected transcript without following any project or
/// bucket symlink. Only the trusted home root itself may be canonicalized.
pub(crate) fn capture_selected_transcript_source(
    home: &Path,
    path: &Path,
    id: uuid::Uuid,
) -> Result<(session::jsonl::writer::TranscriptFileAuthority, String), String> {
    validate_selected_transcript_identity(path, &id.to_string())?;
    let canonical_home =
        std::fs::canonicalize(home).map_err(|_| "selected transcript home is unavailable")?;
    let relative = path
        .strip_prefix(home)
        .or_else(|_| path.strip_prefix(&canonical_home))
        .map_err(|_| "selected transcript is outside the session catalog")?;
    let parts: Vec<_> = relative.components().collect();
    if parts.len() != 3
        || !matches!(parts[0], std::path::Component::Normal(name) if name == "projects")
        || !matches!(parts[1], std::path::Component::Normal(_))
        || !matches!(parts[2], std::path::Component::Normal(_))
    {
        return Err("selected transcript is outside the session catalog".into());
    }
    let (authority, bytes) = session::jsonl::writer::TranscriptFileAuthority::capture(
        canonical_home,
        relative.to_path_buf(),
    )
    .map_err(|error| format!("selected transcript is unavailable: {error}"))?;
    let content = String::from_utf8(bytes).map_err(|_| "selected transcript is not valid UTF-8")?;
    if content.is_empty() {
        return Err("selected transcript must be a nonempty JSONL file".into());
    }
    Ok((authority, content))
}

/// Empty scheduled owners must carry the authoritative workspace anchor.
/// Explicit draft restore also permits a valid, still-unanchored mode-only file.
pub(crate) fn validate_captured_transcript(
    path: &Path,
    id: uuid::Uuid,
    content: &str,
    loaded: &session::jsonl::LoadedTranscript,
    allow_unanchored_draft: bool,
) -> Result<(), String> {
    if !content.ends_with('\n') {
        return Err("selected transcript has an incomplete final record".into());
    }
    if loaded.malformed_line_count != 0 {
        return Err("selected transcript contains malformed records".into());
    }
    let id_text = id.to_string();
    let (messages, _) = session::jsonl::build_conversation_chain(loaded, &id_text);
    if !messages.is_empty() {
        return Ok(());
    }
    let has_anchor_field = content.lines().any(|line| {
        lingxi_core::types::utf16_json::Utf16JsonProjection::parse(line)
            .is_ok_and(|row| row.contains_property("mobileEmptySession"))
    });
    if allow_unanchored_draft
        && !has_anchor_field
        && session::jsonl::is_mode_only_draft(content, &id_text)
    {
        return Ok(());
    }
    let cwd = session::jsonl::validated_empty_draft_cwd(content, &id_text)
        .ok_or("empty session has no valid workspace anchor")?;
    if session::jsonl::loader::transcript_workspace_path(path, loaded)
        .map(PathBuf::from)
        .as_deref()
        != Some(cwd.as_path())
        || !cwd.is_dir()
    {
        return Err("empty session workspace does not match its transcript".into());
    }
    Ok(())
}

/// Bind a selected transcript under its writer authority before configuration,
/// provider assembly, or lifecycle hooks can write a default transcript.
async fn prepare_selected_boot_session(cfg: &mut DesktopConfig) -> Result<(), String> {
    prepare_selected_boot_session_after_capture(cfg, || {}).await
}

async fn prepare_selected_boot_session_after_capture(
    cfg: &mut DesktopConfig,
    after_capture: impl FnOnce() + Send,
) -> Result<(), String> {
    let Some(path) = cfg.session_transcript_path.clone() else {
        return Ok(());
    };
    let id = cfg
        .session_id_override
        .as_deref()
        .ok_or("selected transcript requires a session UUID")?;
    let uuid = validate_selected_transcript_identity(&path, id)?;
    let expected = cfg
        .initial_conversation_mode
        .ok_or("selected transcript requires an expected conversation mode")?;
    // An existing snapshot must prove the same selected file and live lease.
    if let Some(snapshot) = &cfg.session_resume_snapshot {
        let authority = cfg
            .session_transcript_authority
            .as_ref()
            .ok_or("selected transcript snapshot requires file authority")?;
        let lease = cfg
            .session_writer_lease
            .as_ref()
            .ok_or("selected transcript snapshot requires writer authority")?;
        if lease.canonical_session_id().map(|id| id.as_uuid()) != Some(uuid)
            || uuid::Uuid::parse_str(&snapshot.source_session_id).ok() != Some(uuid)
        {
            return Err("selected transcript snapshot has a different session owner".into());
        }
        let (current, content) = capture_selected_transcript_source(&cfg.lingxi_home, &path, uuid)?;
        if &current != authority || content != snapshot.content.as_ref() {
            return Err("selected transcript snapshot source changed".into());
        }
        let fallback;
        let loaded = if let Some(loaded) = cfg.prepared_session_resume() {
            loaded
        } else {
            fallback = session::jsonl::route_lines(&content);
            &fallback
        };
        validate_captured_transcript(&authority.path(), uuid, &content, loaded, cfg.restore_draft)?;
        return validate_selected_session_mode(&content, uuid, expected);
    }
    let (authority, preflight) = capture_selected_transcript_source(&cfg.lingxi_home, &path, uuid)?;
    validate_selected_session_mode(&preflight, uuid, expected)?;
    let preview = session::jsonl::route_lines(&preflight);
    validate_captured_transcript(
        &authority.path(),
        uuid,
        &preflight,
        &preview,
        cfg.restore_draft,
    )?;
    after_capture();
    let lease = if let Some(lease) = cfg.session_writer_lease.as_ref() {
        if lease.canonical_session_id().map(|id| id.as_uuid()) != Some(uuid) {
            return Err(
                "selected transcript writer authority does not match the session UUID".into(),
            );
        }
        lease.clone()
    } else {
        lingxi_core::host::live_sessions::LiveSessionDir::at_live(cfg.lingxi_home.join("sessions"))
            .claim_session_id(&uuid.to_string(), std::process::id())
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::AlreadyExists => {
                    "bridge session id is already active".to_string()
                }
                _ => "bridge session writer authority is unavailable".to_string(),
            })?
            .into_shared()
    };
    let claimed = authority
        .read_checked()
        .map_err(|error| format!("selected transcript changed after writer claim: {error}"))?;
    if claimed != preflight.as_bytes() {
        return Err("selected transcript changed after writer claim".into());
    }
    let content = preflight;
    let path = authority.path();
    // The claim proved both the physical source and every byte unchanged.
    // Reuse the preflight projection while still applying the ordinary loader's
    // identity/native-compact validation. Draft admission remains separate.
    let loaded = preview;
    if !cfg.restore_draft {
        session::jsonl::loader::validate_captured_session_source(&path, uuid, &content, &loaded)
            .map_err(|error| format!("selected transcript is not resumable: {error}"))?;
    }
    validate_captured_transcript(&path, uuid, &content, &loaded, cfg.restore_draft)?;
    // The source can change between the preflight and the writer claim.
    validate_selected_session_mode(&content, uuid, expected)?;
    let (messages, _) = session::jsonl::build_conversation_chain(&loaded, &uuid.to_string());
    if cfg.restore_draft
        && !harness_runtime::desktop::session_state::is_mode_only_draft(&content, &uuid.to_string())
    {
        return Err("draft transcript is not valid mode-only metadata".into());
    }
    if messages.is_empty()
        && !loaded.mobile_empty_sessions.contains(&uuid.to_string())
        && !(cfg.restore_draft
            && harness_runtime::desktop::session_state::is_mode_only_draft(
                &content,
                &uuid.to_string(),
            ))
    {
        return Err("selected transcript has no resumable conversation".into());
    }
    if let Some(workspace) = session::jsonl::loader::transcript_workspace_path(&path, &loaded)
        .map(PathBuf::from)
        .filter(|workspace| workspace.is_dir())
    {
        let workspace = std::fs::canonicalize(workspace)
            .map_err(|_| "selected transcript workspace is unavailable")?;
        let launch = std::fs::canonicalize(&cfg.cwd)
            .map_err(|_| "selected transcript launch directory is unavailable")?;
        if workspace != launch {
            return Err("selected transcript workspace changed; refresh its metadata and trust before launching".into());
        }
    }
    let resume_cost = harness_runtime::desktop::session_state::capture_resume_cost(
        &cfg.lingxi_home,
        uuid,
        lease.clone(),
        &content,
    )
    .await?;
    cfg.session_resume_cost = resume_cost;
    cfg.session_resume_snapshot = Some(harness_runtime::desktop::DesktopSessionResumeSnapshot {
        source_session_id: uuid.to_string(),
        content: content.into(),
    });
    cfg.session_resume_loaded = Some(harness_runtime::desktop::DesktopSessionResumeLoaded::new(
        cfg.session_resume_snapshot
            .as_ref()
            .expect("the claimed capture was just installed"),
        Arc::new(loaded),
    ));
    cfg.session_transcript_path = Some(path);
    cfg.session_transcript_authority = Some(authority);
    cfg.session_writer_lease = Some(lease);
    Ok(())
}

/// Existing drafts are checked before taking writer authority. A missing file
/// is a new draft and its requested mode is established during assembly.
async fn prepare_draft_boot_session(cfg: &mut DesktopConfig) -> Result<(), String> {
    if !cfg.restore_draft {
        return Ok(());
    }
    if cfg.session_transcript_path.is_some() || cfg.session_resume_snapshot.is_some() {
        return Err("draft restore cannot override a selected transcript".into());
    }
    // DesktopConfig uses the same explicit mode for creation and resume
    // admission. Keep it intact for prepare_selected_boot_session and SDK
    // assembly to verify against an existing draft before any startup effects.
    cfg.initial_conversation_mode
        .ok_or("draft restore requires an explicit creation mode")?;
    let uuid = uuid::Uuid::parse_str(
        cfg.session_id_override
            .as_deref()
            .ok_or("draft restore requires a session UUID")?,
    )
    .map_err(|_| "draft restore requires a valid session UUID")?;
    let path = session::jsonl::session_path(
        &cfg.lingxi_home,
        &cfg.cwd.to_string_lossy(),
        &uuid.to_string(),
    );
    match tokio::fs::metadata(&path).await {
        Ok(_) => {
            cfg.session_transcript_path = Some(path);
            return prepare_selected_boot_session(cfg).await;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("draft transcript is unavailable".into()),
    }
    let lease = if let Some(lease) = cfg.session_writer_lease.as_ref() {
        if lease.canonical_session_id().map(|id| id.as_uuid()) != Some(uuid) {
            return Err("draft writer authority does not match the session UUID".into());
        }
        lease.clone()
    } else {
        lingxi_core::host::live_sessions::LiveSessionDir::at_live(cfg.lingxi_home.join("sessions"))
            .claim_session_id(&uuid.to_string(), std::process::id())
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::AlreadyExists => {
                    "bridge session id is already active".to_string()
                }
                _ => "bridge session writer authority is unavailable".to_string(),
            })?
            .into_shared()
    };
    cfg.session_writer_lease = Some(lease);
    // Recheck after the claim in case another owner published this draft
    // between the existence check and acquiring authority.
    match tokio::fs::metadata(&path).await {
        Ok(_) => {
            cfg.session_transcript_path = Some(path);
            prepare_selected_boot_session(cfg).await
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("draft transcript is unavailable".into()),
    }
}

fn initialize_live_session(
    cfg: &mut DesktopConfig,
    receive_gate: Option<Arc<lingxi_core::host::uds_inbox::PeerReceiveGateSlot>>,
) -> Result<LiveSessionGuard, String> {
    let session_id = canonical_session_id(cfg.session_id_override.as_deref())?;
    cfg.session_id_override = Some(session_id.clone());

    let dir =
        lingxi_core::host::live_sessions::LiveSessionDir::at_live(cfg.lingxi_home.join("sessions"));
    let pid = std::process::id();
    // Only live records/PIDs participate in writer ownership. The persisted
    // `<session-id>.jsonl` transcript is intentionally ignored here because a
    // historical resume must reuse that same UUID.
    let writer_claim = match cfg.session_writer_lease.take() {
        Some(lease) if lease.session_id() == session_id => lease,
        Some(_) => {
            return Err("bridge session writer authority does not match its session UUID".into())
        }
        None => dir
            .claim_session_id(&session_id, pid)
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::AlreadyExists => {
                    "bridge session id is already active".to_string()
                }
                _ => "bridge live-session registry is unavailable".to_string(),
            })?
            .into_shared(),
    };
    // Consume this construction-only claim into the desktop config. The
    // coordinator, Fusion recorder, and ordinary transcript writer all clone
    // this exact lease; build must not reacquire a second OS lock.
    cfg.session_writer_lease = Some(writer_claim.clone());

    // Keep the claim in a guard from this point onward. If any later live
    // registration step fails, Drop releases the claim and does not strand a
    // writer lock for the next historical resume.
    let mut live_session = LiveSessionGuard {
        dir: dir.clone(),
        pid,
        session_id: session_id.clone(),
        generation: install_live_session_generation()?,
        inbox_started: false,
        _writer_claim: Some(writer_claim),
    };

    let display_name = cfg
        .cwd
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.trim().is_empty());
    let claim = lingxi_core::host::live_sessions::install_process(
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
            lingxi_core::host::live_sessions::set_process_name(name);
        }
    }

    let permission_mode = cfg.permission_mode.wire_str().to_string();
    lingxi_core::host::live_sessions::set_process_permission_mode(
        &permission_mode,
        cfg.allow_dangerously_skip_permissions,
    );

    let socket = lingxi_core::host::uds_inbox::default_socket_path(pid);
    let inbox = match receive_gate {
        Some(slot) => lingxi_core::host::uds_inbox::start_process_inbox_for_session_with_gate(
            socket,
            &session_id,
            slot,
        ),
        None => lingxi_core::host::uds_inbox::start_process_inbox_for_session(socket, &session_id),
    };
    let inbox_started = match inbox {
        Ok(path) => {
            live_session.inbox_started = true;
            dir.upsert_identity(
                pid,
                &session_id,
                lingxi_core::host::live_sessions::process_name().as_deref(),
                None,
                Some(&path),
                lingxi_core::host::live_sessions::process_permission_class().as_deref(),
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
                lingxi_core::host::live_sessions::process_name().as_deref(),
                None,
                None,
                lingxi_core::host::live_sessions::process_permission_class().as_deref(),
            )
            .map_err(|_| "bridge live-session identity is unavailable".to_string())?;
            false
        }
    };
    lingxi_core::host::live_sessions::set_process_status("idle", None);
    debug_assert_eq!(live_session.inbox_started, inbox_started);
    Ok(live_session)
}

/// Resolve product directories independently of settings, policy, and credentials.
pub fn product_paths_json(cwd: &Path) -> Result<String, String> {
    let brand_home = brand_home().ok_or("product paths require a brand home")?;
    let global_config_path = migrations::global_config::global_config_path();
    product_paths_json_from(cwd, &brand_home, global_config_path.as_deref())
}

fn product_paths_json_from(
    cwd: &Path,
    brand_home: &Path,
    global_config_path: Option<&Path>,
) -> Result<String, String> {
    let brand_home = if brand_home.is_absolute() {
        brand_home.to_path_buf()
    } else {
        cwd.join(brand_home)
    };
    let global_config_path = global_config_path.map(|path| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        }
    });
    serde_json::to_string(&serde_json::json!({
        "version": 2,
        "workspace_root": branding::workspace_home(&brand_home),
        "app_projects_root": branding::workspace_home(&brand_home).join("apps"),
        "brand_home": brand_home,
        "global_config_path": global_config_path,
    }))
    .map_err(|_| "failed to encode product paths".into())
}

/// Read settings-owned environment values before the parent spawns a runtime.
/// This path never resolves providers, credentials, hooks, MCP, or tools.
pub async fn workspace_environment_json(cwd: &Path) -> Result<String, String> {
    let lingxi_home = brand_home().ok_or("workspace environment requires a config home")?;
    let mut cfg = DesktopConfig::default();
    cfg.cwd = cwd.to_path_buf();
    cfg.lingxi_home = lingxi_home.clone();
    let managed = harness_runtime::desktop::managed_settings_overlay(&cfg)
        .await
        .map_err(|_| "workspace environment settings are unavailable")?
        .unwrap_or_else(empty_settings_source);
    workspace_environment_json_from(cwd, &lingxi_home, managed)
}

fn workspace_environment_json_from(
    cwd: &Path,
    lingxi_home: &Path,
    managed: lingxi_core::types::utf16_json::Utf16JsonProjection,
) -> Result<String, String> {
    let snapshot = crate::settings_bridge::build_snapshot(
        &SettingsPaths {
            lingxi_home: lingxi_home.to_path_buf(),
            project_dir: cwd.to_path_buf(),
        },
        empty_settings_source(),
        managed,
    )
    .map_err(|_| "workspace environment settings are unavailable")?;
    if snapshot.files.iter().any(|file| file.parse_error.is_some()) {
        return Err("workspace environment settings are unavailable".into());
    }
    let mut environment = BTreeMap::new();
    if let Some(env) = snapshot
        .effective_source
        .get_property("env")
        .filter(|value| !value.is_null())
    {
        let entries = env
            .object_entries()
            .ok_or("workspace environment must be an object of strings")?;
        for entry in entries {
            let key = String::from_utf16(entry.code_units)
                .map_err(|_| "workspace environment contains an invalid key")?;
            let value = entry
                .value
                .as_str()
                .ok_or("workspace environment contains a non-string value")?;
            if !key.is_empty() && !key.contains(['\0', '=']) && !value.contains('\0') {
                environment.insert(key, value.to_owned());
            }
        }
    }
    let config_home = if lingxi_home.is_absolute() {
        lingxi_home.to_path_buf()
    } else {
        cwd.join(lingxi_home)
    };
    let global_config_path = migrations::global_config::global_config_path().map(|path| {
        if path.is_absolute() {
            path
        } else {
            cwd.join(path)
        }
    });
    serde_json::to_string(&serde_json::json!({
        "version":1, "environment":environment,
        "config_home":config_home, "global_config_path":global_config_path,
    }))
    .map_err(|_| "failed to encode workspace environment".into())
}

/// Enumerate the persisted session catalog without constructing an engine.
///
/// This path deliberately does not call config resolution, credential loading,
/// customization loaders, or `harness_runtime::desktop::build`; it only reads the JSONL
/// catalog rooted at `cwd` and returns the stable Desktop envelope.
///
/// With `session_id` (`--session-id` beside `--list-sessions-json`) the envelope
/// holds at most that session's row, and only its transcript is parsed: Desktop
/// resolves one row before every session it opens.
pub async fn list_sessions_json(cwd: &Path, session_id: Option<&str>) -> Result<String, String> {
    let lingxi_home =
        brand_home().ok_or_else(|| "session catalog requires a config home".to_string())?;
    list_sessions_json_from(cwd, &lingxi_home, session_id).await
}

/// Private Desktop metadata launch-hint catalog. It never constructs an engine.
pub async fn list_sessions_json_in_scope(
    cwd: &Path,
    session_id: Option<&str>,
    scope: session::jsonl::SessionDiscoveryScope,
) -> Result<String, String> {
    let lingxi_home =
        brand_home().ok_or_else(|| "session catalog requires a config home".to_string())?;
    list_sessions_json_from_in_scope(cwd, &lingxi_home, session_id, scope).await
}

/// Describe the exact source selected by Desktop, without choosing a UUID winner.
pub async fn list_selected_session_json_in_scope(
    cwd: &Path,
    session_id: &str,
    source: &Path,
    scope: session::jsonl::SessionDiscoveryScope,
) -> Result<String, String> {
    let lingxi_home =
        brand_home().ok_or_else(|| "session catalog requires a config home".to_string())?;
    list_selected_session_json_from_in_scope(cwd, &lingxi_home, session_id, source, scope).await
}

async fn list_selected_session_json_from_in_scope(
    cwd: &Path,
    lingxi_home: &Path,
    session_id: &str,
    source: &Path,
    scope: session::jsonl::SessionDiscoveryScope,
) -> Result<String, String> {
    let session_id = lingxi_core::types::SessionId::parse_prefixed(session_id)
        .map(|id| id.as_uuid())
        .ok_or_else(|| "session catalog requires a valid session id".to_string())?;
    let described = match session::jsonl::describe_session_source(
        lingxi_home,
        &cwd.to_string_lossy(),
        scope,
        source,
        session_id,
    )
    .await
    {
        Ok(described) => described,
        // A source outside this particular scope may still belong to a wider
        // requested scope. Return no row without resolving the UUID again.
        Err(session::jsonl::LoaderError::SessionNotFound { .. }) => {
            return Ok(serde_json::json!({ "version": 1, "sessions": [] }).to_string());
        }
        Err(error) => {
            tracing::warn!(%error, "bridge-server: selected session source is unavailable");
            return Err("selected session source is unavailable".into());
        }
    };
    if described.metadata.uuid != session_id || described.metadata.path.as_path() != source {
        return Err("selected session source changed".into());
    }
    let row = lower_catalog_row(&described.metadata, described.empty_session)?;
    serde_json::to_string(&serde_json::json!({ "version": 1, "sessions": [row] }))
        .map_err(|_| "failed to encode session catalog".to_string())
}

async fn list_sessions_json_from(
    cwd: &Path,
    lingxi_home: &Path,
    session_id: Option<&str>,
) -> Result<String, String> {
    list_sessions_json_from_with_scope(cwd, lingxi_home, session_id, None).await
}

async fn list_sessions_json_from_in_scope(
    cwd: &Path,
    lingxi_home: &Path,
    session_id: Option<&str>,
    scope: session::jsonl::SessionDiscoveryScope,
) -> Result<String, String> {
    list_sessions_json_from_with_scope(cwd, lingxi_home, session_id, Some(scope)).await
}

async fn list_sessions_json_from_with_scope(
    cwd: &Path,
    lingxi_home: &Path,
    session_id: Option<&str>,
    scope: Option<session::jsonl::SessionDiscoveryScope>,
) -> Result<String, String> {
    let fs: Arc<dyn lingxi_core::host::FileSystem> =
        Arc::new(HostFileSystem::new(cwd.to_path_buf()));
    list_sessions_json_using_fs(cwd, lingxi_home, session_id, scope, fs, None).await
}

async fn list_sessions_json_using_fs(
    cwd: &Path,
    lingxi_home: &Path,
    session_id: Option<&str>,
    scope: Option<session::jsonl::SessionDiscoveryScope>,
    fs: Arc<dyn lingxi_core::host::FileSystem>,
    mut empty_markers: Option<&mut catalog_worker::EmptyMarkerCache>,
) -> Result<String, String> {
    let raw_session_id = session_id;
    let session_id = session_id
        .map(|raw| {
            lingxi_core::types::SessionId::parse_prefixed(raw)
                .map(|parsed| parsed.as_uuid())
                .ok_or_else(|| "session catalog requires a valid session id".to_string())
        })
        .transpose()?;
    let listing = match session_id {
        Some(session_id) => {
            let found = match scope {
                Some(scope) => {
                    session::jsonl::find_session_in_scope_with_diagnostics(
                        lingxi_home,
                        &cwd.to_string_lossy(),
                        scope,
                        session_id,
                        fs.clone(),
                    )
                    .await
                }
                None => {
                    session::jsonl::find_session_exact_raw_with_diagnostics(
                        lingxi_home,
                        &cwd.to_string_lossy(),
                        raw_session_id.expect("a parsed session id retains its spelling"),
                        fs.clone(),
                    )
                    .await
                }
            };
            match found {
                Err(session::jsonl::LoaderError::SessionNotFound { .. }) => {
                    // Mobile's explicit zero-message anchor is a product
                    // extension. Never broaden this exception to arbitrary
                    // metadata-only files or foreign UUID buckets.
                    let path = session::jsonl::session_path(
                        lingxi_home,
                        &cwd.to_string_lossy(),
                        &session_id.to_string(),
                    );
                    let loaded = session::jsonl::JsonlReader::new(path.clone(), fs.clone())
                        .read_routed()
                        .await;
                    match loaded {
                        Ok(loaded)
                            if loaded
                                .mobile_empty_sessions
                                .contains(&session_id.to_string()) =>
                        {
                            session::jsonl::find_session_with_diagnostics(
                                lingxi_home,
                                &cwd.to_string_lossy(),
                                session_id,
                                fs.clone(),
                            )
                            .await
                        }
                        Ok(_) => Ok(session::jsonl::SessionCatalog {
                            sessions: Vec::new(),
                            skipped_files: 0,
                        }),
                        Err(_) if matches!(tokio::fs::try_exists(&path).await, Ok(false)) => {
                            Ok(session::jsonl::SessionCatalog {
                                sessions: Vec::new(),
                                skipped_files: 0,
                            })
                        }
                        Err(error) => Err(session::jsonl::LoaderError::Io {
                            arg: path.display().to_string(),
                            source: std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                error.to_string(),
                            ),
                        }),
                    }
                }
                result => result,
            }
        }
        None => {
            session::jsonl::list_recent_sessions_in_scope_with_diagnostics(
                lingxi_home,
                &cwd.to_string_lossy(),
                scope.unwrap_or_default(),
                usize::MAX,
                fs.clone(),
            )
            .await
        }
    };
    let catalog = match listing {
        Ok(catalog) => catalog,
        Err(session::jsonl::LoaderError::EmptyDirectory) => session::jsonl::SessionCatalog {
            sessions: Vec::new(),
            skipped_files: 0,
        },
        Err(error) => {
            tracing::warn!(%error, "bridge-server: metadata catalog is unavailable");
            return Err("session catalog is unavailable".to_string());
        }
    };
    let mut sessions = Vec::with_capacity(catalog.sessions.len());
    for row in catalog.sessions {
        let empty_session = if row.message_count == 0 {
            if let Some(cache) = empty_markers.as_deref_mut() {
                cache.is_empty(&row, fs.clone()).await
            } else {
                session::jsonl::JsonlReader::new(row.path.clone(), fs.clone())
                    .read_routed()
                    .await
                    .ok()
                    .is_some_and(|loaded| {
                        loaded.messages_in_order.is_empty()
                            && loaded.mobile_empty_sessions.contains(&row.uuid.to_string())
                    })
            }
        } else {
            false
        };
        sessions.push(lower_catalog_row(&row, empty_session)?);
    }
    serde_json::to_string(&serde_json::json!({
        "version": 1,
        "sessions": sessions,
    }))
    .map_err(|_| "failed to encode session catalog".to_string())
}

fn lower_catalog_row(
    row: &session::jsonl::SessionMetadata,
    empty_session: bool,
) -> Result<serde_json::Value, String> {
    let lowered = client::adapter::lowering::lower_session_metadata(row);
    let mut value = serde_json::to_value(lowered)
        .map_err(|_| "failed to encode session catalog".to_string())?;
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "empty_session".to_string(),
            serde_json::Value::Bool(empty_session),
        );
        if let Some(workspace_path) = row.workspace_path.as_deref() {
            object.insert(
                "workspace_path".into(),
                serde_json::Value::String(workspace_path.into()),
            );
        }
        if let Some(title) = row.custom_or_ai_title.as_deref() {
            object.insert(
                "resume_title".into(),
                serde_json::Value::String(title.into()),
            );
        }
        if let Some(model) = row.resume_model.as_deref() {
            object.insert(
                "resume_model".to_string(),
                serde_json::Value::String(lingxi_core::host::qualified_model_ref(
                    model,
                    row.resume_model_profile.as_deref(),
                )),
            );
        }
    }
    Ok(value)
}

/// Assemble a fully-bound [`BridgeConnection`] from a resolved [`DesktopConfig`].
///
/// Wires the connection-scoped sinks into a real [`DesktopRuntime`]
/// (`harness_runtime::desktop::build`), then binds the production
/// [`OrchestratorTurnDriver`] (with an error sink so a turn-level failure
/// reaches the client) and the [`EngineCommandRouter`] over the runtime handles.
///
/// # Errors
/// Returns the [`harness_runtime::desktop::BuildError`] string if the engine cannot be
/// assembled (effectively infallible today).
pub async fn assemble(cfg: DesktopConfig) -> Result<BoundServer, String> {
    Box::pin(assemble_with_account_credentials(
        cfg,
        None,
        BTreeMap::new(),
        None,
        None,
    ))
    .await
}

fn resolve_provider_account_scope(
    home: &std::path::Path,
    snapshot_scope: Option<&str>,
    host_backend: Option<&str>,
    fallback_backend: &str,
) -> Result<String, String> {
    let backend = match host_backend {
        Some(backend @ ("desktop-broker:development" | "desktop-broker:production")) => backend,
        Some(_) => return Err("invalid Host account backend".into()),
        None => fallback_backend,
    };
    let expected = harness_runtime::provider_accounts::account_store_scope(home, backend);
    if host_backend.is_some() && snapshot_scope.is_some_and(|scope| scope != expected) {
        return Err("account store scope mismatch".into());
    }
    Ok(snapshot_scope.unwrap_or(&expected).to_owned())
}

/// Assemble all account routes with private, process-local Host credentials.
pub async fn assemble_with_account_credentials(
    mut cfg: DesktopConfig,
    provider_accounts: Option<lingxi_core::host::provider_accounts::AccountSnapshot>,
    mut provider_account_credentials: BTreeMap<String, secret::AccountCredential>,
    provider_account_home: Option<PathBuf>,
    provider_account_backend: Option<String>,
) -> Result<BoundServer, String> {
    if cfg.team_invocation.is_none() {
        crate::provider_settings::apply_managed(&mut cfg).await?;
    }
    let account_home = provider_account_home.unwrap_or_else(|| cfg.lingxi_home.clone());
    if !account_home.is_absolute() {
        return Err("account home must be an absolute Host path".into());
    }
    crate::provider_accounts::validate_injection(
        provider_accounts.as_ref(),
        &provider_account_credentials,
    )?;
    if let Some(snapshot) = provider_accounts.as_ref() {
        crate::provider_accounts::validate_current_snapshot(&account_home, snapshot)?;
    }
    let media_handoffs =
        Arc::new(harness_runtime::desktop::DesktopMediaAgentHandoffQueue::default());
    cfg.media_agent_handoffs = Some(media_handoffs.clone());
    prepare_draft_boot_session(&mut cfg).await?;
    prepare_selected_boot_session(&mut cfg).await?;
    let cold_resume = if cfg.restore_draft {
        None
    } else {
        crate::workspace_commands::ColdBootResume::capture(&cfg)?
    };
    let provider_credentials_ephemeral = matches!(
        cfg.credential_storage_policy,
        lingxi_core::host::CredentialStoragePolicy::NativeOrMemory
    );
    let enable_automation_scheduler = cfg.enable_automation_scheduler;
    let initial_workspace_trusted = cfg.host_workspace_trusted == Some(true);
    let initial_session_resumed = !cfg.restore_draft && cfg.session_resume_snapshot.is_some();
    let initial_add_dirs = cfg
        .add_dir
        .iter()
        .map(|path| {
            command_api::workspace_directory::resolve_path(
                &path.to_string_lossy(),
                &cfg.cwd,
                dirs::home_dir().as_deref(),
            )
        })
        .collect();
    let receive_gate = Arc::new(lingxi_core::host::uds_inbox::PeerReceiveGateSlot::new());
    let mut live_session = initialize_live_session(&mut cfg, Some(receive_gate.clone()))?;
    let connection = BridgeConnection::new()
        .with_visualization(
            Arc::new(HostFileSystem::new(cfg.cwd.clone())),
            &cfg.lingxi_home,
        )
        .with_private_pages(
            Arc::new(HostFileSystem::new(cfg.cwd.clone())),
            &cfg.lingxi_home,
        );

    // Capture the persisted-session inputs before `cfg` moves into the desktop
    // composition root. The router uses the same cwd/config-home pair as the
    // orchestrator's JSONL writer, so list and resume address the exact store
    // this connection writes.
    let session_store = SessionStoreContext::new(
        cfg.lingxi_home.clone(),
        cfg.cwd.to_string_lossy().into_owned(),
        Arc::new(HostFileSystem::new(cfg.cwd.clone())),
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
    let mut settings_context = {
        let paths = SettingsPaths {
            lingxi_home: cfg.lingxi_home.clone(),
            project_dir: cfg.cwd.clone(),
        };
        // Managed (policy) discovery is the desktop composition root's job;
        // bridge-server does not locate those tiers itself. Resolved once,
        // here, and reused for both `active`'s one-time bake-in below and the
        // `managed` field every later listing re-applies to `effective` — the
        // same map both places, so the two can never drift apart.
        let (managed, managed_diagnostics) =
            match harness_runtime::desktop::managed_settings_overlay(&cfg).await {
                Ok(Some(managed)) => (managed, None),
                Ok(None) => (empty_settings_source(), None),
                Err(lingxi_core::settings::SettingsError::SourceFailure {
                    diagnostics,
                    effective,
                    ..
                }) => (
                    effective.unwrap_or_else(empty_settings_source),
                    Some(diagnostics),
                ),
                Err(error) => return Err(settings_error_message(&error)),
            };
        let active = active_settings_baseline(&paths, &managed)
            .map_err(|error| settings_error_message(&error))?;
        SettingsContext {
            plugin_archive_session: Arc::new(plugin::PluginArchiveSession::new()),
            fusion_layers: Some(Arc::new({
                let config = cfg.clone_for_runtime_controls();
                // Settings reloads outlive individual session mounts. The
                // bootstrap lease belongs to the hydrated session, not this
                // configuration snapshot.
                move || harness_runtime::desktop::fusion_settings_layers_for_config(&config)
            })),
            paths,
            active: Arc::new(std::sync::RwLock::new(active)),
            managed,
            managed_diagnostics,
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
    let cron_firer = Arc::new(crate::cron_host::HostCronFirer::new(
        event_sink.clone(),
        cfg.cwd.clone(),
        connection.cron_requests_handle(),
    ));
    // Subagent lifecycle/message events use the same authenticated sink as the
    // main turn. `initialize_live_session` canonicalizes this id before this
    // function is entered, so the observer and engine transcript share one
    // session fence even when the host did not provide an id explicitly.
    let session_agent_observer = Arc::new(
        harness_runtime::desktop::session_agents::DesktopSessionAgentObserver::new(
            event_sink.clone(),
            cfg.session_id_override.clone().unwrap_or_default(),
        ),
    );
    cfg.session_agent_observer = Some(session_agent_observer.clone());
    let message_output = client::adapter::AdapterOutputStream::new(event_sink.clone())
        .with_turn_terminal_deferrer(media_handoffs.clone());
    let output: Arc<dyn OutputStream> = Arc::new(message_output.clone());
    let permission_sink = connection.permission_sink();

    // The `computer` tool's `request_access` approval — the Electron-facing
    // sibling of the permission round-trip above. A fresh channel: the SENDER
    // half fills `cfg.computer_access_tx`, which `harness_runtime::desktop::build` wires
    // into the GENERIC `tool_computer_use::TuiBridgeResolver` (the same
    // resolver the TUI host uses — see its own doc comment for why it is
    // transport-agnostic); the RECEIVER half is drained by a connection-scoped
    // `ComputerAccessBroker`, which lowers each exchange into a
    // `Frame::ComputerAccessRequest` push and parks the reply channel keyed by
    // a fresh `request_id`, exactly mirroring the permission gate's shape.
    let (computer_access_tx, computer_access_rx) =
        tokio::sync::mpsc::channel::<permission::computer_access::ComputerAccessExchange>(8);
    cfg.computer_access_tx = Some(computer_access_tx);
    let computer_access_broker = Arc::new(client::adapter::ComputerAccessBroker::new(
        connection.computer_access_sink(),
    ));
    // Native audio and media-device ports are both projected over this one
    // authenticated Bridge connection. The shared MediaService owns routing,
    // owner-bound sessions and storage; its low-level audio backend and media
    // device callback both reach the same Electron-native coordinator.
    let (decision_service, decision_responder) =
        crate::decision_bridge::new_decision_bridge(connection.audio_sink());
    cfg.decision_service = Some(decision_service);
    let (audio_bridge, audio_responder) = new_audio_bridge(connection.audio_sink());
    let (media_device_callback, media_device_responder) =
        new_media_device_callback(connection.audio_sink());
    let media_ports = Arc::new(CallbackMediaDevicePorts::new(media_device_callback));
    let media_resources = Arc::new(BoundedMediaResourceStore::default());
    let cloud_audio = Arc::new(AudioProviderCloudRegistry::new("wav"));
    let media_host = AppMediaHost::new(
        media_resources,
        audio_bridge.clone(),
        None,
        media_ports,
        Vec::new(),
        cloud_audio.clone(),
        Vec::new(),
        media_runtime::AudioRoutingSettings::default(),
        1,
        client::protocol::media::MAX_MEDIA_DEVICE_FRAME_BYTES as u64,
    )
    .map_err(|error| format!("could not compose the app-scoped media host: {error}"))?;
    // initialize_live_session canonicalized the configured session before
    // DesktopRuntime is built; use that same ID as the initial media owner.
    let media_owner = MediaOwner::Session {
        session_id: cfg
            .session_id_override
            .clone()
            .ok_or_else(|| "bridge session ID was not initialized before media setup".to_owned())?,
    };
    let media_attachment = media_host
        .create_media_attachment()
        .map_err(|error| format!("could not create the Bridge media attachment: {error}"))?;
    let mut media_attachment_guard =
        BootMediaAttachmentGuard::new(media_attachment.clone(), cloud_audio.clone());
    media_host
        .set_current_media_attachment(Some(media_attachment.clone()))
        .map_err(|error| format!("could not publish the Bridge media attachment: {error}"))?;
    let media_configuration_host = media_host.clone();
    let media_configuration_snapshot: audio_provider::AudioConfigurationSnapshot =
        Arc::new(move || media_configuration_host.audio_configuration_json());
    cfg.media_conversation_attachment =
        Some(AppMediaConversationAttachment::new_for_attachment_slot(
            media_host.cloud_audio_registry(),
            media_host.media_attachment_slot(),
            media_configuration_snapshot,
        ));
    cfg.media_service = Some(media_host.media_service());
    // Computer Use uses the app host's attachment-bound AudioService facade;
    // it shares routing with MediaService without bypassing owner teardown.
    cfg.audio = Some(DesktopAudio {
        service: media_host.desktop_audio_service(),
    });
    let (media_connection_channel, media_connection_responder) =
        connection.media_connection_channel();
    let (media_control_channel, media_control_responder) = connection.media_control_channel();
    let remote_media_connector = remote_control_client::host_media::RemoteMediaConnector::new(
        Arc::new(DesktopRemoteMediaConfigProvider::from_process_environment()),
        media_host.resource_store(),
    );
    media_host.install_connector(remote_media_connector.clone(), []);
    let (ask_user_question_tx, ask_user_question_rx) =
        tokio::sync::mpsc::channel::<tool_api::ask_user_question::AskUserQuestionExchange>(8);
    cfg.ask_user_question_tx = Some(ask_user_question_tx);
    let ask_user_question_broker = Arc::new(client::adapter::AskUserQuestionBroker::new(
        connection.event_sink(),
    ));
    let official_catalog = cfg.startup_catalog.clone();
    let provider_model_catalog_listings = desktop_provider_catalog_listings(
        &cfg,
        harness_runtime::desktop::resolve_provider_region(&cfg, &settings_context.managed)
            .map_err(|error| settings_error_message(&error))?,
    )
    .await;

    let mut shared = harness_runtime::desktop::build_shared_credential_stack_with_policy(
        &cfg.lingxi_home,
        cfg.isolated_credential_storage,
        cfg.credential_storage_policy,
    )
    .await
    .map_err(|error| error.to_string())?;
    if provider_credentials_ephemeral {
        // The parent is the only provider-key authority. Keep the shared native
        // store for MCP, while provider authentication belongs to the account store.
        shared.credentials.use_host_managed_provider_keys();
    }
    if let Some(snapshot) = provider_accounts {
        crate::provider_accounts::validate_current_snapshot(&account_home, &snapshot)?;
        for account in &snapshot.accounts {
            shared
                .credentials
                .set_account_credential_ephemeral(
                    &account.credential_ref,
                    provider_account_credentials.remove(&account.account_id),
                )
                .await
                .map_err(|_| "failed to install private account credential".to_string())?;
        }
        shared.provider_accounts = Some(snapshot);
    }
    shared.provider_account_home = Some(account_home.clone());
    let account_scope = resolve_provider_account_scope(
        &account_home,
        shared
            .provider_accounts
            .as_ref()
            .map(|s| s.store_scope.as_str()),
        provider_account_backend.as_deref(),
        &format!("{:?}", shared.credentials.account_storage_backend()),
    )?;
    let account_catalog_source = cfg.startup_catalog.clone();
    let account_catalog_inputs = provider_config::AssembleInputs {
        official_catalog: None,
        anthropic_api_base: cfg.api_base.clone(),
        anthropic_models: llm_runtime::builtin_presets()
            .providers
            .into_iter()
            .find(|profile| profile.profile_name == "anthropic")
            .map(|profile| profile.models)
            .unwrap_or_default(),
        anthropic_has_api_key: false,
        anthropic_has_oauth: false,
        user_providers: cfg.provider_profiles.clone().unwrap_or_default(),
        routing: cfg.routing.clone(),
    };
    let provider_account_store = harness_runtime::provider_accounts::ProviderAccountStore::new(
        account_home.clone(),
        account_scope,
        shared.credentials.clone(),
    );
    if provider_credentials_ephemeral {
        shared
            .credentials
            .set_account_credential_refresher(Arc::new(
                crate::provider_accounts::BrokerAccountRefresher {
                    data_root: account_home,
                    credentials: Arc::downgrade(&shared.credentials),
                    sink: event_sink.clone(),
                    locks: Default::default(),
                },
            ));
    }
    let initial_account_snapshot = shared.provider_accounts.clone();
    let runtime = Arc::new(
        // Do not nest the SDK's large assembly future in every transport wrapper.
        // This also keeps cold resume within ordinary worker-thread stack limits.
        Box::pin(harness_runtime::desktop::build_with_host_automation(
            cfg,
            output,
            permission_sink,
            shared,
        ))
        .await
        .map_err(|e| e.to_string())?,
    );
    // Replace the lazy placeholder with the exact session used by desktop
    // discovery and PluginManager before the context is attached to the Router.
    settings_context.plugin_archive_session = runtime.plugin_archive_session.clone();
    let workspace_commands = crate::workspace_commands::register(
        &runtime,
        &settings_context,
        session_store.lingxi_home().to_path_buf(),
        initial_workspace_trusted,
        initial_add_dirs,
        initial_session_resumed,
    )
    .await;
    workspace_commands.bind_cold_resume(cold_resume);
    receive_gate.set(runtime.orchestrator.clone());
    // The runtime cloned the exact construction-only lease into its hydrated
    // coordinator. Do not let the bridge's process-registration guard pin the
    // old session through later hot clear/resume operations.
    live_session.release_writer_claim();
    let provider_account_store = Arc::new({
        provider_account_store.with_catalog(
            harness_runtime::provider_accounts::ProviderAccountCatalog::new(
                account_catalog_inputs,
                runtime.provider_region,
                account_catalog_source,
                None,
            ),
        )
    });
    let provider_region = match runtime.provider_region {
        llm_runtime::Region::ChinaMainland => "china_mainland",
        llm_runtime::Region::International => "international",
    };
    settings_context
        .active
        .write()
        .map_err(|_| "settings state lock poisoned".to_owned())?
        .set_field(
            "providerRegion",
            lingxi_core::types::utf16_json::Utf16JsonProjection::from(provider_region),
        )
        .map_err(|error| format!("failed to update active providerRegion: {error}"))?;

    // `use_noop_permission_gate: false` ⇒ build MUST surface the adapter gate.
    let gate = runtime.permission_gate.clone().ok_or_else(|| {
        "harness_runtime::desktop::build did not surface an AdapterPermissionGate despite \
         use_noop_permission_gate=false"
            .to_string()
    })?;
    gate.set_session_id(Some(
        runtime
            .orchestrator
            .current_session_id()
            .await
            .as_uuid()
            .to_string(),
    ));

    // §27 mid-turn drain + Now-abort wiring. The per-connection queue is shared
    // (Arc) across THREE consumers: (1) the orchestrator's mid-turn input source
    // (injects queued `Next`/`Now` prompts WITHIN a running turn), (2) the turn
    // driver (registers each turn's cancel token so a `Now` enqueue aborts it),
    // and (3) the existing between-turn `drain_main_thread` loop. A
    // `CancelReasonFlag` is shared between the orchestrator (reads it to tell a
    // `Now`-command abort from a user interrupt) and the queue's now-abort hook
    // (sets it to `QueueNowCommand` right before firing the active-turn token).
    let queue = connection.queue_handle();
    runtime
        .mod_command_catalog
        .bind_queue(connection.mod_command_queue());
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
                        scheduled_task_id: None,
                        scheduled_fire_id: None,
                        uuid: message.message_id,
                        content: msgqueue::QueuedCommandContent::UserInput {
                            text: harness_runtime::desktop::teammate_message_envelope_with_summary(
                                &message.from_name,
                                &message.content,
                                message.summary.as_ref(),
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
    // `Later` so it waits for the active turn and never enters the mid-turn drain. The cell was
    // threaded out of `harness_runtime::desktop::build` on `DesktopRuntime` precisely because
    // the tool is constructed before this seam. Setting it more than once is a no-op
    // (`OnceLock`); a fresh per-connection `assemble` builds a fresh runtime + cell.
    let wakeup_scheduler = Arc::new(
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
        .with_orchestrator(runtime.orchestrator.clone())
        .with_event_sink(connection.loop_wakeup_event_sink()),
    );
    // The driver re-uses the SAME scheduler at its turn-completion edge to arm the
    // `/loop` keepalive fallback (binary `lKi`); clone before the cell consumes it.
    if let Some(cron) = &runtime.session_lifecycle.cron_scheduler {
        cron.set_session_delivery(wakeup_scheduler.clone()).await;
    }
    let driver_wakeup_scheduler = wakeup_scheduler.clone();
    if runtime.wakeup_scheduler_cell.set(wakeup_scheduler).is_err() {
        // Already filled — should not happen for a fresh runtime, but never panic
        // at the composition root over a benign double-wire.
    }

    // A turn with no usable credential is NOT special-cased here. It runs, the
    // provider rejects it, and `orchestrator::api_error_copy` renders upstream's
    // own auth copy — which is what claude-code does, and what the desktop
    // already knows how to display. Measured on a credential-less boot: the
    // terminal frames arrive in 41ms with no retry backoff.
    let driver: Arc<dyn TurnDriver> = Arc::new(
        OrchestratorTurnDriver::with_error_sink(runtime.orchestrator.clone(), event_sink)
            .with_desktop_runtime(runtime.clone())
            .with_workspace_session_facts(workspace_commands.session_facts())
            .with_message_output(message_output)
            .with_queue(queue, cancel_reason)
            .with_wakeup_scheduler(driver_wakeup_scheduler)
            .with_foreign_input_counter(foreign_input_counter),
    );

    if enable_automation_scheduler {
        if let Some(scheduler) = &runtime.session_lifecycle.cron_scheduler {
            scheduler.set_automation_firer(cron_firer.clone()).await;
        }
    }

    let (audio_services, audio_credential_ids, _) = runtime.audio_provider_services();
    let audio_provider = Arc::new(AudioProviderHost::from_provider_services(
        audio_services,
        audio_credential_ids.clone(),
    ));
    let realtime_audio = Arc::new(
        crate::realtime_audio::RealtimeAudioController::new(
            runtime.orchestrator.clone(),
            audio_provider.clone(),
        )
        .with_execution_owners(connection.realtime_execution_owners(
            gate.clone(),
            computer_access_broker.clone(),
            ask_user_question_broker.clone(),
        )),
    );
    // Preserve app-shared explicit-profile routes for non-session owners.
    // Attached conversation sessions additionally receive an exact provider
    // entry below and never fall back to this table.
    for profile_name in audio_credential_ids.keys() {
        if let Err(error) =
            cloud_audio.register_profile(profile_name.clone(), audio_provider.clone())
        {
            tracing::warn!(%error, "could not register a configured provider profile for media audio");
        }
    }
    let mut initial_media_audio_binding = None;
    if let Ok((session_id, profile_name)) = realtime_audio.session_context().await {
        let owner = MediaOwner::Session {
            session_id: session_id.clone(),
        };
        match media_attachment.authorize_owner(&owner) {
            Ok(permit) => {
                for profile_name in audio_credential_ids.keys() {
                    if let Err(error) = cloud_audio.register_attached_profile(
                        &permit,
                        profile_name.clone(),
                        audio_provider.clone(),
                    ) {
                        tracing::warn!(%error, "could not register an attachment-scoped media audio profile");
                    }
                }
                match cloud_audio.register_attached_session(
                    &permit,
                    profile_name.clone(),
                    lingxi_core::host::provider_accounts::UNBOUND_ACCOUNT_SCOPE,
                    audio_provider.clone(),
                ) {
                    Ok(generation) => {
                        initial_media_audio_binding =
                            Some((media_attachment.clone(), session_id, generation));
                    }
                    Err(error) => {
                        tracing::warn!(%error, "could not register the current desktop attachment for media audio");
                    }
                }
            }
            Err(error) => {
                tracing::warn!(%error, "could not authorize the current desktop attachment for media audio");
            }
        }
    }

    // The full command-routing seam over the real engine handles.
    let handle: Arc<dyn OrchestratorHandle> = runtime.orchestrator.clone();
    let dispatcher: Arc<dyn SlashCommandDispatcher> = Arc::new(
        runtime
            .dispatcher
            .clone_shared()
            .with_injected_messages_as_turns(),
    );
    let mut router = EngineCommandRouter::new(
        handle,
        runtime.task_registry.clone()
            as Arc<dyn lingxi_core::host::task_registry::TaskRegistryHandle>,
        Some(dispatcher),
        Some(runtime.shared_command_registry.clone()),
    )
    .with_workspace_commands(workspace_commands)
    .with_side_chat_runtime(
        runtime.orchestrator.clone(),
        runtime.side_chat_factory.clone(),
    )
    .with_cron_firer(cron_firer)
    .with_session_cron(runtime.session_lifecycle.cron_scheduler.clone())
    .with_provider_account_store(provider_account_store, initial_account_snapshot)
    .with_catalog_registry(runtime.catalog_registry.clone())
    .with_provider_model_catalog_listings(provider_model_catalog_listings)
    .with_official_catalog(official_catalog)
    .with_ephemeral_provider_credentials(provider_credentials_ephemeral)
    .with_session_store(
        session_store
            .with_runtime_cwd(runtime.session_cwd.clone())
            .with_transcript_writer(runtime.orchestrator.session_transcript_writer()),
    )
    .with_session_agent_observer(session_agent_observer)
    .with_team_registry(runtime.coordinator.clone())
    .with_settings_context(settings_context)
    .with_mcp_paths(mcp_paths)
    .with_mcp_registry(runtime.mcp_registry.clone())
    .with_plugin_runtime(runtime.plugin_runtime.clone())
    .with_hook_registry(runtime.hook_registry.clone())
    .with_repo_root_reloader(runtime.repo_root_reloader.clone())
    .with_file_changed_watcher(runtime.file_changed_watcher.controller());
    router = router
        .with_realtime_audio(realtime_audio)
        .with_media_cloud_audio_registry(
            cloud_audio.clone(),
            media_host.media_attachment_slot(),
            audio_provider.clone(),
            initial_media_audio_binding,
        );
    let router = Arc::new(router);
    spawn_mcp_startup_publisher(
        runtime.watch_mcp_startup(),
        router.clone(),
        connection.loop_wakeup_event_sink(),
    );

    let media_service_rpc = MediaServiceRpc::for_app_host_with_remote_connector(
        media_host.clone(),
        media_owner,
        remote_media_connector,
        media_connection_channel.clone(),
        media_control_channel.clone(),
    )
    .map_err(|error| format!("could not bind the Bridge media owner attachment: {error}"))?;
    let connection = connection
        .with_policy_helper_refusal(runtime.policy_helper_refusal.clone())
        .bind(gate, driver)
        .bind_router(router)
        .bind_computer_access(computer_access_broker, computer_access_rx)
        .bind_ask_user_question(ask_user_question_broker, ask_user_question_rx)
        // The response half of the SAME pair whose request half went into
        // `cfg.audio` above — this is what makes an inbound `AudioResponse` on
        // this connection resolve a call the engine parked on this connection,
        // and a disconnect drain them.
        .bind_decision(decision_responder)
        .bind_audio(audio_responder)
        .bind_media_device(media_device_responder)
        .bind_media_connection(media_connection_responder)
        .bind_media_control(media_control_responder)
        .bind_media_service(media_service_rpc);
    let connection = connection
        .with_task_notification_registry(runtime.task_registry.clone())
        .with_queue_wakeup();
    media_attachment_guard.disarm();
    Ok(BoundServer {
        connection,
        runtime,
        audio: audio_bridge,
        media_host,
        media_connection_channel,
        media_control_channel,
        live_session,
    })
}

/// Startup MCP servers connect after the session opens
/// (`DesktopConfig::connect_mcp_in_background`). Re-publish the MCP listings
/// each time one settles so a client that fetched them on connect sees the
/// servers, their status and their prompt commands arrive. The sink is the
/// connection's unscoped one: these updates belong to no turn.
fn spawn_mcp_startup_publisher(
    mut progress: tokio::sync::watch::Receiver<harness_runtime::desktop::McpStartupProgress>,
    router: Arc<EngineCommandRouter>,
    sink: Arc<dyn client::adapter::ClientEventSink>,
) {
    // Waiting for startup progress must not keep a closed session (and its
    // writer authority) alive. Hold the router only while publishing an update.
    let router = Arc::downgrade(&router);
    tokio::spawn(async move {
        while progress.changed().await.is_ok() {
            let settled = progress.borrow_and_update().pending == 0;
            let Some(router) = router.upgrade() else {
                break;
            };
            router.publish_mcp_startup_progress(sink.as_ref()).await;
            if settled {
                break;
            }
        }
    });
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

/// Lockfile publication failure that returns ownership of the already-bound
/// endpoint to the host.  The bridge must stop and join that endpoint before
/// draining durable session state; dropping it here would only signal the
/// accept loop and would not prove that accepted connections have finished.
pub struct PublishLockfileError {
    source: std::io::Error,
    endpoint: McpEndpoint,
}

impl PublishLockfileError {
    /// Recover the error and endpoint for ordered host teardown.
    #[must_use]
    pub fn into_parts(self) -> (std::io::Error, McpEndpoint) {
        (self.source, self.endpoint)
    }
}

impl std::fmt::Debug for PublishLockfileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PublishLockfileError")
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Display for PublishLockfileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.source.fmt(formatter)
    }
}

impl std::error::Error for PublishLockfileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
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
    publish_lockfile_recoverable(endpoint, lockfile_dir, workspace_folders)
        .map_err(|error| error.into_parts().0)
}

/// [`publish_lockfile`] variant for a production host that must retain the
/// bound endpoint when the filesystem write fails so teardown can join every
/// already-accepted connection before releasing session authority.
pub fn publish_lockfile_recoverable(
    endpoint: McpEndpoint,
    lockfile_dir: PathBuf,
    workspace_folders: Vec<PathBuf>,
) -> Result<ServedEndpoint, PublishLockfileError> {
    let port = endpoint.port();
    let lockfile = IdeLockfile::new_for_bridge_dir(lockfile_dir, port, workspace_folders);
    let lockfile_path = lockfile.path();
    if let Err(source) = lockfile.write() {
        return Err(PublishLockfileError { source, endpoint });
    }
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
pub(crate) async fn close_unserved_for_test(bound: &BoundServer) {
    let attachment = bound
        .media_host
        .current_media_attachment()
        .expect("assembled bridge has a media attachment");
    bridge::FramePump::on_close(&bound.connection).await;
    assert_eq!(
        attachment.phase(),
        media_runtime::MediaAttachmentPhase::Revoked,
        "the unserved bridge must finish its real media cleanup"
    );
    assert!(bound
        .media_host
        .media_attachment_host()
        .attachment(attachment.id())
        .is_none());
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn failed_boot_guard_closes_and_drains_its_attachment() {
        use media_runtime::{MediaAttachmentHost, MediaAttachmentPhase, MediaRuntimeBuilder};
        use std::{sync::Arc, time::Duration};
        let host = MediaAttachmentHost::new();
        let service = MediaRuntimeBuilder::new(Arc::new(
            audio_provider::media_store::BoundedMediaResourceStore::default(),
        ))
        .build();
        let attachment = host.attach(service).unwrap();
        let cloud = Arc::new(audio_provider::media_cloud::AudioProviderCloudRegistry::new("wav"));
        drop(super::BootMediaAttachmentGuard::new(
            attachment.clone(),
            cloud,
        ));
        assert_ne!(attachment.phase(), MediaAttachmentPhase::Active);
        assert_eq!(
            tokio::time::timeout(
                Duration::from_secs(3),
                attachment.wait_for_drain_completion()
            )
            .await
            .unwrap(),
            MediaAttachmentPhase::Revoked
        );
        assert!(host.attachment(attachment.id()).is_none());
    }
    use super::*;
    use lingxi_core::host::live_sessions::LiveSessionDir;

    #[test]
    fn remote_timeout_flags_are_validated_and_reach_the_runtime() {
        let args = BridgeArgs::parse([
            "--permission-timeout-secs",
            "1800",
            "--ask-user-question-timeout",
            "never",
        ])
        .unwrap();
        let config = resolve_desktop_config(&args).unwrap();
        assert_eq!(
            config.resolved_permission_timeout().unwrap(),
            std::time::Duration::from_secs(1800)
        );
        assert_eq!(
            config.ask_user_question_timeout_override,
            Some(tool_ui::ask_user_question::AskUserQuestionTimeout::Never)
        );
        assert!(resolve_desktop_config(&BridgeArgs::default())
            .unwrap()
            .permission_timeout
            .is_none());
        for value in ["0", "86401", "-1", "1.5", "18446744073709551616"] {
            assert!(BridgeArgs::parse(["--permission-timeout-secs", value]).is_err());
        }
        for value in ["", "forever", "1800"] {
            assert!(BridgeArgs::parse(["--ask-user-question-timeout", value]).is_err());
        }
        assert!(BridgeArgs::parse(["--permission-timeout-secs"]).is_err());
        assert!(BridgeArgs::parse(["--ask-user-question-timeout"]).is_err());
    }

    #[test]
    fn parse_defaults_when_no_args() {
        let args = BridgeArgs::parse(Vec::<String>::new()).expect("empty parses");
        assert_eq!(args, BridgeArgs::default());
        assert!(!args.help);
    }

    #[test]
    fn product_paths_mode_rejects_runtime_and_other_discovery_flags() {
        let args = BridgeArgs::parse(["--product-paths-json", "--cwd", "/tmp/product"])
            .expect("product discovery accepts its directory");
        assert!(args.product_paths_json);
        for flags in [
            vec!["--product-paths-json", "--workspace-environment-json"],
            vec!["--product-paths-json", "--list-sessions-json"],
            vec!["--product-paths-json", "--model", "unused"],
            vec!["--product-paths-json", "--credential-stdin"],
            vec!["--product-paths-json", "--trusted-workspace"],
        ] {
            assert!(BridgeArgs::parse(flags).is_err());
        }
    }

    #[test]
    fn product_paths_ignore_invalid_settings_and_retain_the_product_namespace() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join(".another-product");
        std::fs::create_dir(&home).unwrap();
        let settings = home.join("settings.json");
        std::fs::write(&settings, "invalid settings").unwrap();
        let paths: serde_json::Value =
            serde_json::from_str(&product_paths_json_from(root.path(), &home, None).unwrap())
                .unwrap();
        assert_eq!(
            paths,
            serde_json::json!({"version": 2, "brand_home": home, "workspace_root": branding::workspace_home(&home), "app_projects_root": branding::workspace_home(&home).join("apps"), "global_config_path": null})
        );
        assert_eq!(
            std::fs::read_to_string(settings).unwrap(),
            "invalid settings"
        );
        assert!(!home.join(".apps").exists());
    }

    #[test]
    fn product_paths_resolve_relative_overrides_against_the_discovery_directory() {
        let root = tempfile::tempdir().unwrap();
        let paths: serde_json::Value = serde_json::from_str(
            &product_paths_json_from(
                root.path(),
                Path::new("custom-product"),
                Some(Path::new("custom-product/config.json")),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            paths["brand_home"],
            serde_json::json!(root.path().join("custom-product"))
        );
        assert_eq!(
            paths["app_projects_root"],
            serde_json::json!(root.path().join("custom-product-workspaces/apps"))
        );
        assert_eq!(
            paths["global_config_path"],
            serde_json::json!(root.path().join("custom-product/config.json"))
        );
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

    #[test]
    fn selected_workspace_boot_path_is_private_absolute_and_matches_the_stable_uuid() {
        let id = "11111111-2222-4333-8444-555555abcdef";
        let source = format!("/persisted source/{}.jsonl", id.to_uppercase());
        let args = BridgeArgs::parse([
            "--session-id",
            id,
            "--session-transcript-path",
            &source,
            "--expected-conversation-mode",
            "code",
        ])
        .unwrap();
        assert_eq!(args.session_transcript_path, Some(PathBuf::from(&source)));
        for flags in [
            vec!["--session-transcript-path", source.as_str()],
            vec![
                "--session-id",
                id,
                "--session-transcript-path",
                "relative.jsonl",
            ],
            vec![
                "--session-id",
                id,
                "--session-transcript-path",
                "/11111111-2222-4333-8444-555555555555.jsonl",
            ],
            vec![
                "--session-id",
                id,
                "--session-transcript-path",
                source.as_str(),
                "--list-sessions-json",
            ],
        ] {
            assert!(BridgeArgs::parse(flags).is_err());
        }
    }

    #[test]
    fn selected_catalog_lookup_requires_an_exact_id_and_explicit_scope() {
        let id = "11111111-2222-4333-8444-555555abcdef";
        let source = format!("/catalog/{}.jsonl", id.to_uppercase());
        let args = BridgeArgs::parse([
            "--list-sessions-json",
            "--session-id",
            id,
            "--session-transcript-path",
            &source,
            "--session-scope",
            "current",
        ])
        .unwrap();
        assert_eq!(args.session_transcript_path, Some(PathBuf::from(&source)));
        assert_eq!(
            args.session_scope,
            Some(session::jsonl::SessionDiscoveryScope::Current)
        );
        for flags in [
            vec![
                "--list-sessions-json",
                "--session-transcript-path",
                source.as_str(),
                "--session-scope",
                "all",
            ],
            vec![
                "--list-sessions-json",
                "--session-id",
                id,
                "--session-transcript-path",
                source.as_str(),
            ],
            vec![
                "--list-sessions-json",
                "--session-id",
                id,
                "--session-transcript-path",
                "/catalog/11111111-2222-4333-8444-555555555555.jsonl",
                "--session-scope",
                "all",
            ],
            vec![
                "--list-sessions-json",
                "--session-id",
                id,
                "--session-transcript-path",
                source.as_str(),
                "--session-scope",
                "all",
                "--expected-conversation-mode",
                "code",
            ],
        ] {
            assert!(BridgeArgs::parse(flags).is_err());
        }
    }

    #[test]
    fn draft_restore_flag_requires_an_owned_runtime_session() {
        let id = "11111111-2222-4333-8444-555555abcdef";
        let parsed = BridgeArgs::parse([
            "--session-id",
            id,
            "--restore-draft",
            "--initial-conversation-mode",
            "code",
        ])
        .unwrap();
        assert!(parsed.restore_draft);
        assert!(resolve_desktop_config(&parsed).unwrap().restore_draft);
        assert!(BridgeArgs::parse(["--restore-draft"]).is_err());
        assert!(BridgeArgs::parse([
            "--session-id",
            id,
            "--restore-draft",
            "--list-sessions-json"
        ])
        .is_err());
        assert!(BridgeArgs::parse([
            "--session-id",
            id,
            "--restore-draft",
            "--session-transcript-path",
            &format!("/tmp/{id}.jsonl")
        ])
        .is_err());
    }

    #[test]
    fn durable_draft_title_requires_a_new_owned_draft() {
        let id = "11111111-2222-4333-8444-555555abcdef";
        for mode in ["chat", "code"] {
            let args = BridgeArgs::parse([
                "--session-id",
                id,
                "--restore-draft",
                "--initial-conversation-mode",
                mode,
                "--durable-draft-title",
                "Local App",
            ])
            .unwrap();
            assert_eq!(args.durable_draft_title.as_deref(), Some("Local App"));
            assert_eq!(
                resolve_desktop_config(&args)
                    .unwrap()
                    .durable_draft_title
                    .as_deref(),
                Some("Local App")
            );
        }
        for flags in [
            vec!["--durable-draft-title"],
            vec!["--durable-draft-title", "Local App"],
            vec!["--session-id", id, "--durable-draft-title", "Local App"],
            vec![
                "--restore-draft",
                "--initial-conversation-mode",
                "code",
                "--durable-draft-title",
                "Local App",
            ],
            vec![
                "--session-id",
                id,
                "--restore-draft",
                "--durable-draft-title",
                "Local App",
            ],
            vec![
                "--session-id",
                id,
                "--restore-draft",
                "--initial-conversation-mode",
                "code",
                "--expected-conversation-mode",
                "chat",
                "--durable-draft-title",
                "Local App",
            ],
            vec![
                "--session-id",
                id,
                "--restore-draft",
                "--initial-conversation-mode",
                "chat",
                "--list-sessions-json",
                "--durable-draft-title",
                "Local App",
            ],
            vec![
                "--session-id",
                id,
                "--restore-draft",
                "--initial-conversation-mode",
                "code",
                "--durable-draft-title",
                "  ",
            ],
            vec![
                "--session-id",
                id,
                "--restore-draft",
                "--initial-conversation-mode",
                "code",
                "--scheduled-durable-anchor",
            ],
        ] {
            assert!(BridgeArgs::parse(flags).is_err());
        }
        assert!(resolve_desktop_config(&BridgeArgs::default())
            .unwrap()
            .durable_draft_title
            .is_none());
    }

    #[test]
    fn initial_conversation_mode_is_a_private_owned_draft_argument() {
        let id = "11111111-2222-4333-8444-555555abcdef";
        for (value, expected) in [
            ("chat", ConversationMode::Chat),
            ("code", ConversationMode::Code),
        ] {
            let parsed = BridgeArgs::parse([
                "--session-id",
                id,
                "--restore-draft",
                "--initial-conversation-mode",
                value,
            ])
            .unwrap();
            assert_eq!(parsed.initial_conversation_mode, Some(expected));
            assert_eq!(
                resolve_desktop_config(&parsed)
                    .unwrap()
                    .initial_conversation_mode,
                Some(expected)
            );
        }
        for flags in [
            vec!["--initial-conversation-mode", "chat"],
            vec!["--session-id", id, "--initial-conversation-mode", "chat"],
            vec![
                "--session-id",
                id,
                "--restore-draft",
                "--initial-conversation-mode",
            ],
            vec![
                "--session-id",
                id,
                "--restore-draft",
                "--initial-conversation-mode",
                "invalid",
            ],
            vec![
                "--session-id",
                id,
                "--restore-draft",
                "--initial-conversation-mode",
                "chat",
                "--list-sessions-json",
            ],
        ] {
            assert!(BridgeArgs::parse(flags).is_err());
        }
        assert_eq!(
            resolve_desktop_config(&BridgeArgs::default())
                .unwrap()
                .initial_conversation_mode,
            None
        );
    }

    #[test]
    fn draft_initial_and_expected_modes_must_agree() {
        let id = "11111111-2222-4333-8444-555555abcdef";
        for (initial, expected) in [("code", "chat"), ("chat", "code")] {
            let error = BridgeArgs::parse([
                "--session-id",
                id,
                "--restore-draft",
                "--initial-conversation-mode",
                initial,
                "--expected-conversation-mode",
                expected,
            ])
            .unwrap_err();
            assert!(error.contains("initial and expected conversation modes must match"));
        }
        for (value, mode) in [
            ("chat", ConversationMode::Chat),
            ("code", ConversationMode::Code),
        ] {
            let parsed = BridgeArgs::parse([
                "--session-id",
                id,
                "--restore-draft",
                "--initial-conversation-mode",
                value,
                "--expected-conversation-mode",
                value,
            ])
            .unwrap();
            assert_eq!(parsed.initial_conversation_mode, Some(mode));
            assert_eq!(parsed.expected_conversation_mode, Some(mode));
            assert_eq!(
                resolve_desktop_config(&parsed)
                    .unwrap()
                    .initial_conversation_mode,
                Some(mode)
            );
        }
    }

    #[tokio::test]
    async fn draft_boot_captures_valid_metadata_under_the_session_writer_lease() {
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().join("workspace");
        std::fs::create_dir(&cwd).unwrap();
        let home = root.path().join("home");
        let id = "11111111-2222-4333-8444-555555abcdef";
        let path = session::jsonl::session_path(&home, &cwd.to_string_lossy(), id);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let content = format!(
            "{}\n",
            serde_json::json!({"type":"fixed-session-mode", "sessionId":id,
            "sessionMode":"chat", "instructionEpoch":3})
        );
        std::fs::write(&path, &content).unwrap();
        let mut cfg = DesktopConfig::default();
        cfg.cwd = cwd;
        cfg.lingxi_home = home.clone();
        cfg.session_id_override = Some(id.into());
        cfg.restore_draft = true;
        cfg.initial_conversation_mode = Some(ConversationMode::Chat);
        prepare_draft_boot_session(&mut cfg).await.unwrap();
        assert_eq!(
            cfg.session_transcript_path.as_ref(),
            Some(&path.canonicalize().unwrap())
        );
        assert_eq!(
            cfg.session_resume_snapshot
                .as_ref()
                .unwrap()
                .content
                .as_ref(),
            content
        );
        assert_eq!(cfg.session_writer_lease.as_ref().unwrap().session_id(), id);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
        drop(cfg);
        assert!(LiveSessionDir::at_live(home.join("sessions"))
            .claim_session_id(id, std::process::id())
            .is_ok());
    }

    #[tokio::test]
    async fn draft_boot_rejects_corrupt_or_message_bearing_files_without_changing_them() {
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().join("workspace");
        std::fs::create_dir(&cwd).unwrap();
        let home = root.path().join("home");
        let id = "11111111-2222-4333-8444-555555abcdef";
        let path = session::jsonl::session_path(&home, &cwd.to_string_lossy(), id);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mode = serde_json::json!({"type":"fixed-session-mode", "sessionId":id,
            "sessionMode":"chat", "instructionEpoch":3});
        for content in [
            format!("{mode}\n{{broken\n"),
            format!(
                "{mode}\n{}\n",
                serde_json::json!({"type":"user","sessionId":id,"message":{}})
            ),
        ] {
            std::fs::write(&path, &content).unwrap();
            let mut cfg = DesktopConfig::default();
            cfg.cwd = cwd.clone();
            cfg.lingxi_home = home.clone();
            cfg.session_id_override = Some(id.into());
            cfg.restore_draft = true;
            cfg.initial_conversation_mode = Some(ConversationMode::Chat);
            assert!(prepare_draft_boot_session(&mut cfg).await.is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
        }
    }

    #[tokio::test]
    async fn new_draft_without_a_transcript_remains_new_with_a_writer_lease() {
        let root = tempfile::tempdir().unwrap();
        let mut cfg = DesktopConfig::default();
        cfg.cwd = root.path().into();
        cfg.lingxi_home = root.path().join("home");
        cfg.session_id_override = Some("11111111-2222-4333-8444-555555abcdef".into());
        cfg.restore_draft = true;
        cfg.initial_conversation_mode = Some(ConversationMode::Code);
        prepare_draft_boot_session(&mut cfg).await.unwrap();
        assert!(cfg.session_transcript_path.is_none());
        assert!(cfg.session_resume_snapshot.is_none());
        assert!(cfg.session_writer_lease.is_some());
    }

    #[test]
    fn private_catalog_scope_is_explicit_and_catalog_only() {
        use session::jsonl::SessionDiscoveryScope as Scope;
        for (value, scope) in [
            ("current", Scope::Current),
            ("repository", Scope::Repository),
            ("all", Scope::All),
        ] {
            let args =
                BridgeArgs::parse(["--list-sessions-json", "--session-scope", value]).unwrap();
            assert_eq!(args.session_scope, Some(scope));
        }
        assert!(BridgeArgs::parse(["--session-scope", "all"]).is_err());
        assert!(BridgeArgs::parse(["--list-sessions-json", "--session-scope", "unknown"]).is_err());
        let args = BridgeArgs::parse(["--add-dir", "/one space", "--add-dir", "/目录"]).unwrap();
        assert_eq!(
            args.add_dir,
            vec![PathBuf::from("/one space"), PathBuf::from("/目录")]
        );
        let args = BridgeArgs::parse(["--session-project-root", "/初始 workspace"]).unwrap();
        assert_eq!(
            args.session_project_root,
            Some(PathBuf::from("/初始 workspace"))
        );
        assert!(BridgeArgs::parse(["--session-project-root", "relative"]).is_err());
        assert!(BridgeArgs::parse(["--session-project-root"]).is_err());
        let owners = BridgeArgs::parse([
            "--session-config-home",
            "/identity/config",
            "--session-global-config-path",
            "/identity/global.json",
        ])
        .unwrap();
        assert_eq!(
            owners.session_config_home.as_deref(),
            Some(Path::new("/identity/config"))
        );
        assert_eq!(
            owners.session_global_config_path.as_deref(),
            Some(Path::new("/identity/global.json"))
        );
        assert!(BridgeArgs::parse(["--session-config-home", "relative"]).is_err());
        assert!(BridgeArgs::parse(["--session-global-config-path", "relative"]).is_err());
    }

    #[test]
    fn workspace_environment_metadata_merges_all_sources_and_filters_os_invalid_entries() {
        use migrations::settings_update::{settings_path, WritableScope};
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().join("workspace");
        let home = root.path().join("config");
        for (scope, value) in [
            (
                WritableScope::User,
                serde_json::json!({"env":{"USER_ONLY":"user", "SAME":"user"}}),
            ),
            (
                WritableScope::Project,
                serde_json::json!({"env":{"PROJECT_ONLY":"project", "SAME":"project"}}),
            ),
            (
                WritableScope::Local,
                serde_json::json!({"env":{"LOCAL_ONLY":"local", "SAME":"local", "INVALID=KEY":"omitted", "NUL":"bad\0value"}}),
            ),
        ] {
            let path = settings_path(scope, &home, &cwd);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, value.to_string()).unwrap();
        }
        let managed = lingxi_core::types::utf16_json::Utf16JsonProjection::parse(
            r#"{"env":{"MANAGED_ONLY":"managed","SAME":"managed"}}"#,
        )
        .unwrap();
        let result: serde_json::Value =
            serde_json::from_str(&workspace_environment_json_from(&cwd, &home, managed).unwrap())
                .unwrap();
        assert_eq!(
            result,
            serde_json::json!({
                "version":1,
                "config_home":home,
                "global_config_path":migrations::global_config::global_config_path().map(|path| if path.is_absolute() {path} else {cwd.join(path)}),
                "environment":{
                    "USER_ONLY":"user", "PROJECT_ONLY":"project", "LOCAL_ONLY":"local", "MANAGED_ONLY":"managed", "SAME":"managed",
                },
            })
        );
        assert!(
            !home.join("projects").exists(),
            "environment metadata must not create a session store"
        );
    }

    #[test]
    fn workspace_environment_metadata_rejects_non_string_values_without_echoing_them() {
        let root = tempfile::tempdir().unwrap();
        let source = lingxi_core::types::utf16_json::Utf16JsonProjection::parse(
            r#"{"env":{"DO_NOT_ECHO":{"secret":"credential-like-value"}}}"#,
        )
        .unwrap();
        let error = workspace_environment_json_from(root.path(), root.path(), source).unwrap_err();
        assert!(!error.contains("DO_NOT_ECHO") && !error.contains("credential-like-value"));
    }

    fn fixed_catalog_header(id: &str) -> serde_json::Value {
        serde_json::json!({
            "type":"fixed-session-mode", "sessionId":id,
            "sessionMode":"code", "instructionEpoch":0,
        })
    }

    fn write_workspace_catalog_row(home: &Path, cwd: &Path, id: &str) -> PathBuf {
        let path = session::jsonl::session_path(home, &cwd.to_string_lossy(), id);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let header = fixed_catalog_header(id);
        let row = serde_json::json!({
            "type":"user", "uuid":"aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee", "parentUuid":null,
            "sessionId":id, "timestamp":"2026-10-21T00:00:00.000Z", "cwd":cwd,
            "isSidechain":false, "message":{"role":"user","content":"workspace catalog"},
        });
        std::fs::write(&path, format!("{header}\n{row}\n")).unwrap();
        path
    }

    #[tokio::test]
    async fn selected_boot_workspace_captures_exact_source_without_mutating_copies() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let cwd = root.path().join("trusted workspace");
        std::fs::create_dir(&cwd).unwrap();
        let id = "11111111-2222-4333-8444-555555abcdef";
        let default_path = write_workspace_catalog_row(&home, &cwd, id);
        let default_bytes = std::fs::read(&default_path).unwrap();
        let selected = home
            .join("projects/selected-source")
            .join(format!("{}.jsonl", id.to_uppercase()));
        std::fs::create_dir_all(selected.parent().unwrap()).unwrap();
        let selected_content = String::from_utf8(default_bytes.clone())
            .unwrap()
            .replace("workspace catalog", "exact selected source B");
        std::fs::write(&selected, &selected_content).unwrap();
        let mut cfg = DesktopConfig::default();
        cfg.cwd = cwd.clone();
        cfg.lingxi_home = home.clone();
        cfg.session_id_override = Some(id.into());
        cfg.session_transcript_path = Some(selected.clone());
        cfg.initial_conversation_mode = Some(ConversationMode::Code);
        prepare_selected_boot_session(&mut cfg).await.unwrap();
        assert_eq!(cfg.cwd, cwd);
        assert_eq!(
            cfg.session_transcript_path.as_ref(),
            Some(&std::fs::canonicalize(&selected).unwrap())
        );
        assert!(cfg.session_transcript_authority.is_some());
        let snapshot = cfg.session_resume_snapshot.as_ref().unwrap();
        assert_eq!(snapshot.source_session_id, id);
        assert_eq!(snapshot.content.as_ref(), selected_content);
        let prepared = cfg
            .prepared_session_resume()
            .expect("boot retains its validated parse");
        let prepared_address = prepared as *const session::jsonl::LoadedTranscript;
        let proof = crate::workspace_commands::ColdBootResume::capture(&cfg).unwrap();
        assert!(proof.is_some());
        assert_eq!(
            cfg.prepared_session_resume().unwrap() as *const _,
            prepared_address
        );
        prepare_selected_boot_session(&mut cfg).await.unwrap();
        assert_eq!(
            cfg.prepared_session_resume().unwrap() as *const _,
            prepared_address,
            "revalidating an unchanged writer capture must reuse the same parsed source"
        );
        assert_eq!(cfg.session_writer_lease.as_ref().unwrap().session_id(), id);
        assert_eq!(std::fs::read(&default_path).unwrap(), default_bytes);
        assert_eq!(
            std::fs::read_to_string(&selected).unwrap(),
            selected_content
        );
        drop(cfg);
        assert!(
            LiveSessionDir::at_live(home.join("sessions"))
                .claim_session_id(id, std::process::id())
                .is_ok(),
            "failed boot preparation must not strand a lease"
        );
    }

    #[tokio::test]
    async fn selected_boot_rejects_mode_mismatch_before_writer_claim() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let cwd = root.path().join("workspace");
        std::fs::create_dir(&cwd).unwrap();
        let id = "11111111-2222-4333-8444-555555abcdef";
        let path = write_workspace_catalog_row(&home, &cwd, id);
        let original = std::fs::read(&path).unwrap();
        let mut cfg = DesktopConfig::default();
        cfg.cwd = cwd;
        cfg.lingxi_home = home.clone();
        cfg.session_id_override = Some(id.into());
        cfg.session_transcript_path = Some(path.clone());
        cfg.initial_conversation_mode = Some(ConversationMode::Chat);
        let error = prepare_selected_boot_session(&mut cfg).await.unwrap_err();
        assert!(error.contains("session_mode_mismatch"));
        assert!(cfg.session_writer_lease.is_none());
        assert!(!home.join("sessions").exists());
        assert_eq!(std::fs::read(path).unwrap(), original);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn selected_boot_rejects_linked_catalog_bucket_before_claim() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let cwd = root.path().join("workspace");
        std::fs::create_dir_all(home.join("projects")).unwrap();
        std::fs::create_dir(&cwd).unwrap();
        let id = "11111111-2222-4333-8444-555555abcdef";
        let outside_home = root.path().join("outside");
        let outside = write_workspace_catalog_row(&outside_home, &cwd, id);
        let original = std::fs::read(&outside).unwrap();
        symlink(outside.parent().unwrap(), home.join("projects/linked")).unwrap();
        let selected = home.join("projects/linked").join(format!("{id}.jsonl"));
        let mut cfg = DesktopConfig::default();
        cfg.cwd = cwd;
        cfg.lingxi_home = home.clone();
        cfg.session_id_override = Some(id.into());
        cfg.session_transcript_path = Some(selected);
        cfg.initial_conversation_mode = Some(ConversationMode::Code);
        assert!(prepare_selected_boot_session(&mut cfg).await.is_err());
        assert!(cfg.session_writer_lease.is_none());
        assert!(!home.join("sessions").exists());
        assert_eq!(std::fs::read(outside).unwrap(), original);
    }

    #[tokio::test]
    async fn selected_boot_snapshot_requires_matching_source_authority() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let cwd = root.path().join("workspace");
        std::fs::create_dir(&cwd).unwrap();
        let id = "11111111-2222-4333-8444-555555abcdef";
        let selected = write_workspace_catalog_row(&home, &cwd, id);
        let content = std::fs::read_to_string(&selected).unwrap();
        let mut cfg = DesktopConfig::default();
        cfg.cwd = cwd;
        cfg.lingxi_home = home.clone();
        cfg.session_id_override = Some(id.into());
        cfg.session_transcript_path = Some(selected);
        cfg.initial_conversation_mode = Some(ConversationMode::Code);
        cfg.session_resume_snapshot =
            Some(harness_runtime::desktop::DesktopSessionResumeSnapshot {
                source_session_id: id.into(),
                content: content.clone().into(),
            });
        assert!(prepare_selected_boot_session(&mut cfg).await.is_err());
        assert!(cfg.session_writer_lease.is_none());
        assert!(!home.join("sessions").exists());
    }

    #[tokio::test]
    async fn selected_boot_rejects_same_bytes_replaced_after_capture() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let cwd = root.path().join("workspace");
        std::fs::create_dir(&cwd).unwrap();
        let id = "11111111-2222-4333-8444-555555abcdef";
        let selected = write_workspace_catalog_row(&home, &cwd, id);
        let original = std::fs::read(&selected).unwrap();
        let replacement = selected.with_file_name("replacement.jsonl");
        let retired = selected.with_file_name("retired.jsonl");
        std::fs::write(&replacement, &original).unwrap();
        let mut cfg = DesktopConfig::default();
        cfg.cwd = cwd;
        cfg.lingxi_home = home.clone();
        cfg.session_id_override = Some(id.into());
        cfg.session_transcript_path = Some(selected.clone());
        cfg.initial_conversation_mode = Some(ConversationMode::Code);
        let selected_for_swap = selected.clone();
        let retired_for_swap = retired.clone();
        let result = prepare_selected_boot_session_after_capture(&mut cfg, move || {
            std::fs::rename(&selected_for_swap, &retired_for_swap).unwrap();
            std::fs::rename(&replacement, &selected_for_swap).unwrap();
        })
        .await;
        assert!(result.is_err());
        assert!(cfg.session_writer_lease.is_none());
        assert!(cfg.session_transcript_authority.is_none());
        assert_eq!(std::fs::read(&selected).unwrap(), original);
        assert_eq!(std::fs::read(retired).unwrap(), original);
        assert!(LiveSessionDir::at_live(home.join("sessions"))
            .claim_session_id(id, std::process::id())
            .is_ok());
    }

    #[tokio::test]
    async fn selected_boot_rejects_complete_json_without_record_terminator() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let cwd = root.path().join("workspace");
        std::fs::create_dir(&cwd).unwrap();
        let id = "11111111-2222-4333-8444-555555abcdef";
        let selected = write_workspace_catalog_row(&home, &cwd, id);
        let mut content = std::fs::read_to_string(&selected).unwrap();
        assert_eq!(content.pop(), Some('\n'));
        std::fs::write(&selected, &content).unwrap();
        let mut cfg = DesktopConfig::default();
        cfg.cwd = cwd;
        cfg.lingxi_home = home.clone();
        cfg.session_id_override = Some(id.into());
        cfg.session_transcript_path = Some(selected.clone());
        cfg.initial_conversation_mode = Some(ConversationMode::Code);
        let error = prepare_selected_boot_session(&mut cfg).await.unwrap_err();
        assert!(error.contains("incomplete final record"), "{error}");
        assert!(cfg.session_writer_lease.is_none());
        assert!(!home.join("sessions").exists());
        assert_eq!(std::fs::read_to_string(selected).unwrap(), content);
    }

    #[tokio::test]
    async fn selected_empty_session_rejects_missing_anchor_cwd_before_claim() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let cwd = root.path().join("workspace");
        std::fs::create_dir(&cwd).unwrap();
        let id = "11111111-2222-4333-8444-555555abcdef";
        let path = session::jsonl::session_path(&home, &cwd.to_string_lossy(), id);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let content = format!(
            "{}\n{}\n",
            serde_json::json!({"type":"fixed-session-mode","sessionId":id,"sessionMode":"code","instructionEpoch":0}),
            serde_json::json!({"type":"custom-title","sessionId":id,"customTitle":"scheduled","mobileEmptySession":1})
        );
        std::fs::write(&path, &content).unwrap();
        let loaded = session::jsonl::route_lines(&content);
        assert!(validate_captured_transcript(
            &path,
            uuid::Uuid::parse_str(id).unwrap(),
            &content,
            &loaded,
            false
        )
        .is_err());
        let mut cfg = DesktopConfig::default();
        cfg.cwd = cwd;
        cfg.lingxi_home = home.clone();
        cfg.session_id_override = Some(id.into());
        cfg.session_transcript_path = Some(path.clone());
        cfg.initial_conversation_mode = Some(ConversationMode::Code);
        assert!(prepare_selected_boot_session(&mut cfg).await.is_err());
        assert!(cfg.session_writer_lease.is_none());
        assert!(!home.join("sessions").exists());
        assert_eq!(std::fs::read_to_string(path).unwrap(), content);
    }

    #[tokio::test]
    async fn selected_empty_session_accepts_matching_durable_workspace_anchor() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let cwd = root.path().join("workspace");
        std::fs::create_dir(&cwd).unwrap();
        let id = "11111111-2222-4333-8444-555555abcdef";
        let path = session::jsonl::session_path(&home, &cwd.to_string_lossy(), id);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let content = format!(
            "{}\n{}\n",
            serde_json::json!({"type":"fixed-session-mode","sessionId":id,"sessionMode":"code","instructionEpoch":0}),
            serde_json::json!({"type":"custom-title","sessionId":id,"customTitle":"scheduled","mobileEmptySession":1,"cwd":cwd})
        );
        std::fs::write(&path, &content).unwrap();
        let mut cfg = DesktopConfig::default();
        cfg.cwd = cwd;
        cfg.lingxi_home = home;
        cfg.session_id_override = Some(id.into());
        cfg.session_transcript_path = Some(path.clone());
        cfg.initial_conversation_mode = Some(ConversationMode::Code);
        prepare_selected_boot_session(&mut cfg).await.unwrap();
        assert!(cfg.session_transcript_authority.is_some());
        assert!(cfg.session_writer_lease.is_some());
        assert_eq!(std::fs::read_to_string(path).unwrap(), content);
    }

    #[tokio::test]
    async fn selected_boot_workspace_rejects_missing_empty_and_wrong_uuid_before_registry() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let id = "11111111-2222-4333-8444-555555abcdef";
        let selected = root.path().join(format!("{id}.jsonl"));
        let mut cfg = DesktopConfig::default();
        cfg.cwd = root.path().into();
        cfg.lingxi_home = home.clone();
        cfg.session_id_override = Some(id.into());
        cfg.session_transcript_path = Some(selected.clone());
        cfg.initial_conversation_mode = Some(ConversationMode::Code);
        assert!(prepare_selected_boot_session(&mut cfg).await.is_err());
        assert!(!home.exists());
        std::fs::write(&selected, "").unwrap();
        assert!(prepare_selected_boot_session(&mut cfg).await.is_err());
        assert!(!home.exists());
        cfg.session_transcript_path = Some(
            root.path()
                .join("11111111-2222-4333-8444-555555555555.jsonl"),
        );
        assert!(prepare_selected_boot_session(&mut cfg).await.is_err());
        assert!(!home.exists());
        assert!(cfg.session_writer_lease.is_none());
        assert!(cfg.session_resume_snapshot.is_none());
    }

    #[tokio::test]
    async fn selected_boot_workspace_rejects_stale_existing_workspace_and_keeps_missing_fallback() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let launch = root.path().join("trusted launch");
        let recorded = root.path().join("recorded workspace");
        std::fs::create_dir(&launch).unwrap();
        std::fs::create_dir(&recorded).unwrap();
        let id = "11111111-2222-4333-8444-555555abcdef";
        let selected = write_workspace_catalog_row(&home, &recorded, id);
        let original = std::fs::read(&selected).unwrap();
        let mut cfg = DesktopConfig::default();
        cfg.cwd = launch.clone();
        cfg.lingxi_home = home.clone();
        cfg.session_id_override = Some(id.into());
        cfg.session_transcript_path = Some(selected.clone());
        cfg.initial_conversation_mode = Some(ConversationMode::Code);
        let error = prepare_selected_boot_session(&mut cfg).await.unwrap_err();
        assert!(error.contains("refresh its metadata and trust"));
        assert!(cfg.session_writer_lease.is_none());
        assert!(cfg.session_resume_snapshot.is_none());
        assert_eq!(std::fs::read(&selected).unwrap(), original);
        std::fs::remove_dir(&recorded).unwrap();
        prepare_selected_boot_session(&mut cfg).await.unwrap();
        assert_eq!(cfg.cwd, launch);
        assert_eq!(
            cfg.session_transcript_path.as_ref(),
            Some(&std::fs::canonicalize(&selected).unwrap())
        );
        assert!(cfg.session_transcript_authority.is_some());
        assert_eq!(
            cfg.session_resume_snapshot
                .as_ref()
                .unwrap()
                .content
                .as_bytes(),
            original
        );
        assert!(!session::jsonl::session_path(&home, &cfg.cwd.to_string_lossy(), id).exists());
    }

    #[tokio::test]
    async fn catalog_scopes_and_exact_lookup_keep_authoritative_workspace_hints_private() {
        use session::jsonl::SessionDiscoveryScope as Scope;
        let root = tempfile::tempdir().unwrap();
        let canonical_root = root.path().canonicalize().unwrap();
        let home = canonical_root.join("home");
        let current = canonical_root.join("current workspace");
        let other = canonical_root.join("另一目录");
        std::fs::create_dir_all(&current).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let current_id = "11111111-2222-4333-8444-555555555555";
        let other_id = "66666666-7777-4888-8999-aaaaaaaaaaaa";
        let source = write_workspace_catalog_row(&home, &current, current_id);
        write_workspace_catalog_row(&home, &other, other_id);
        let current_json: serde_json::Value = serde_json::from_str(
            &list_sessions_json_from(&current, &home, None)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(current_json["sessions"].as_array().unwrap().len(), 1);
        assert_eq!(
            current_json["sessions"][0]["workspace_path"],
            current.to_string_lossy().as_ref()
        );
        let all_json: serde_json::Value = serde_json::from_str(
            &list_sessions_json_from_in_scope(&current, &home, None, Scope::All)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(all_json["sessions"].as_array().unwrap().len(), 2);
        let exact: serde_json::Value = serde_json::from_str(
            &list_sessions_json_from(&current, &home, Some(other_id))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            exact["sessions"][0]["workspace_path"],
            other.to_string_lossy().as_ref()
        );
        assert_eq!(exact["sessions"][0]["uuid"], other_id);

        let scoped: serde_json::Value = serde_json::from_str(
            &list_sessions_json_from_in_scope(&current, &home, Some(other_id), Scope::Current)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(scoped["sessions"].as_array().unwrap().len(), 0);
        let scoped: serde_json::Value = serde_json::from_str(
            &list_sessions_json_from_in_scope(&current, &home, Some(other_id), Scope::All)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(scoped["sessions"][0]["uuid"], other_id);

        session::jsonl::record_session_alias(&home, source.parent().unwrap(), &other)
            .await
            .unwrap();
        let local: serde_json::Value =
            serde_json::from_str(&list_sessions_json_from(&other, &home, None).await.unwrap())
                .unwrap();
        assert_eq!(local["sessions"].as_array().unwrap().len(), 2);
        let aliased: serde_json::Value = serde_json::from_str(
            &list_sessions_json_from_in_scope(&other, &home, None, Scope::Repository)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(aliased["sessions"].as_array().unwrap().len(), 2);
        let reverse: serde_json::Value = serde_json::from_str(
            &list_sessions_json_from(&current, &home, None)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(reverse["sessions"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn selected_catalog_source_preserves_same_uuid_copy_and_scope() {
        use session::jsonl::SessionDiscoveryScope as Scope;
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let home = root.join("home");
        let current = root.join("current");
        let other = root.join("other");
        std::fs::create_dir_all(&current).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let id = "11111111-2222-4333-8444-555555abcdef";
        let source_a = write_workspace_catalog_row(&home, &current, id);
        let source_b = write_workspace_catalog_row(&home, &other, id);
        let a_bytes = std::fs::read(&source_a).unwrap();
        let b_content = std::fs::read_to_string(&source_b)
            .unwrap()
            .replace("workspace catalog", "selected other source");
        std::fs::write(&source_b, &b_content).unwrap();

        let current_row: serde_json::Value = serde_json::from_str(
            &list_selected_session_json_from_in_scope(
                &current,
                &home,
                id,
                &source_a,
                Scope::Current,
            )
            .await
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            current_row["sessions"][0]["path"],
            source_a.to_str().unwrap()
        );
        assert_eq!(current_row["sessions"][0]["mode"], "code");
        assert_eq!(current_row["sessions"][0]["empty_session"], false);
        let outside_current: serde_json::Value = serde_json::from_str(
            &list_selected_session_json_from_in_scope(
                &current,
                &home,
                id,
                &source_b,
                Scope::Current,
            )
            .await
            .unwrap(),
        )
        .unwrap();
        assert!(outside_current["sessions"].as_array().unwrap().is_empty());

        let other_row: serde_json::Value = serde_json::from_str(
            &list_selected_session_json_from_in_scope(&current, &home, id, &source_b, Scope::All)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(other_row["sessions"].as_array().unwrap().len(), 1);
        assert_eq!(other_row["sessions"][0]["path"], source_b.to_str().unwrap());
        assert_eq!(
            other_row["sessions"][0]["workspace_path"],
            other.to_str().unwrap()
        );
        assert_eq!(other_row["sessions"][0]["title"], "selected other source");
        assert_eq!(std::fs::read_to_string(&source_b).unwrap(), b_content);

        session::jsonl::record_session_alias(&home, source_b.parent().unwrap(), &current)
            .await
            .unwrap();
        let linked_row: serde_json::Value = serde_json::from_str(
            &list_selected_session_json_from_in_scope(
                &current,
                &home,
                id,
                &source_b,
                Scope::Current,
            )
            .await
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            linked_row["sessions"][0]["path"],
            source_b.to_str().unwrap()
        );

        std::fs::remove_file(&source_b).unwrap();
        assert!(list_selected_session_json_from_in_scope(
            &current,
            &home,
            id,
            &source_b,
            Scope::All
        )
        .await
        .is_err());
        assert_eq!(std::fs::read(&source_a).unwrap(), a_bytes);
    }

    #[tokio::test]
    async fn selected_catalog_source_validates_empty_anchor_without_writes() {
        use session::jsonl::SessionDiscoveryScope as Scope;
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let home = root.join("home");
        let cwd = root.join("workspace");
        std::fs::create_dir_all(&cwd).unwrap();
        let id = "11111111-2222-4333-8444-555555abcdef";
        let path = session::jsonl::session_path(&home, &cwd.to_string_lossy(), id);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let content = format!(
            "{}\n{}\n",
            serde_json::json!({"type":"fixed-session-mode","sessionId":id,"sessionMode":"code","instructionEpoch":0}),
            serde_json::json!({"type":"custom-title","sessionId":id,"customTitle":"scheduled","mobileEmptySession":1})
        );
        std::fs::write(&path, &content).unwrap();
        assert!(
            list_selected_session_json_from_in_scope(&cwd, &home, id, &path, Scope::Current)
                .await
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
        assert!(!home.join("sessions").exists());

        let anchored = format!(
            "{}\n{}\n",
            serde_json::json!({"type":"fixed-session-mode","sessionId":id,"sessionMode":"code","instructionEpoch":0}),
            serde_json::json!({"type":"custom-title","sessionId":id,"customTitle":"scheduled","mobileEmptySession":1,"cwd":cwd})
        );
        std::fs::write(&path, &anchored).unwrap();
        let row: serde_json::Value = serde_json::from_str(
            &list_selected_session_json_from_in_scope(&cwd, &home, id, &path, Scope::Current)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(row["sessions"][0]["path"], path.to_str().unwrap());
        assert_eq!(row["sessions"][0]["empty_session"], true);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), anchored);
        assert!(!home.join("sessions").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn selected_catalog_source_rejects_linked_bucket() {
        use session::jsonl::SessionDiscoveryScope as Scope;
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let home = root.join("home");
        let outside = root.join("outside");
        let cwd = root.join("workspace");
        std::fs::create_dir_all(&cwd).unwrap();
        let id = "11111111-2222-4333-8444-555555abcdef";
        let real = write_workspace_catalog_row(&outside, &cwd, id);
        let original = std::fs::read(&real).unwrap();
        let projects = home.join("projects");
        std::fs::create_dir_all(&projects).unwrap();
        let linked_bucket = projects.join(real.parent().unwrap().file_name().unwrap());
        std::os::unix::fs::symlink(real.parent().unwrap(), &linked_bucket).unwrap();
        let selected = linked_bucket.join(format!("{id}.jsonl"));
        assert!(
            list_selected_session_json_from_in_scope(&cwd, &home, id, &selected, Scope::All)
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(real).unwrap(), original);
        assert!(!home.join("sessions").exists());
    }

    #[tokio::test]
    async fn catalog_resume_title_keeps_full_custom_title_separate_from_display_summary() {
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().join("workspace");
        let home = root.path().join("home");
        std::fs::create_dir_all(&cwd).unwrap();
        let id = "11111111-2222-4333-8444-555555555555";
        let path = write_workspace_catalog_row(&home, &cwd, id);
        let title = format!(" {} 完整标题 ", "title".repeat(90));
        let mut source = std::fs::read_to_string(&path).unwrap();
        source.push_str(&format!(
            "{}\n",
            serde_json::json!({
                "type":"custom-title", "sessionId":id, "customTitle":title,
            })
        ));
        std::fs::write(&path, source).unwrap();
        let catalog: serde_json::Value =
            serde_json::from_str(&list_sessions_json_from(&cwd, &home, None).await.unwrap())
                .unwrap();
        assert_eq!(catalog["sessions"][0]["resume_title"], title);
        assert!(
            catalog["sessions"][0]["resume_title"]
                .as_str()
                .unwrap()
                .len()
                > 400
        );
    }

    #[tokio::test]
    async fn catalog_launch_hint_uses_relocation_instead_of_the_old_message_cwd() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let old = root.path().join("old");
        let target = root.path().join("target");
        std::fs::create_dir_all(&target).unwrap();
        let id = "11111111-2222-4333-8444-555555555555";
        let original = write_workspace_catalog_row(&home, &old, id);
        let moved = session::jsonl::session_path(&home, &target.to_string_lossy(), id);
        std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
        std::fs::rename(original, &moved).unwrap();
        let mut contents = std::fs::read_to_string(&moved).unwrap();
        contents.push_str(&format!(
            "{}\n",
            serde_json::json!({"type":"relocated","sessionId":id,"relocatedCwd":target})
        ));
        std::fs::write(moved, contents).unwrap();
        let exact: serde_json::Value = serde_json::from_str(
            &list_sessions_json_from(&target, &home, Some(id))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            exact["sessions"][0]["workspace_path"],
            target.to_string_lossy().as_ref()
        );
    }

    #[tokio::test]
    async fn list_mode_is_empty_and_does_not_create_catalog_files() {
        let cwd = tempfile::tempdir().expect("cwd tempdir");
        let home = tempfile::tempdir().expect("config tempdir");
        let json = list_sessions_json_from(cwd.path(), home.path(), None)
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
        let historical_header = fixed_catalog_header(historical_id);
        std::fs::write(
            &historical_path,
            format!("{historical_header}\n{system_line}\n"),
        )
        .expect("write historical row");
        let empty_path =
            session::jsonl::session_path(home.path(), &cwd.path().to_string_lossy(), empty_id);
        let empty_line = serde_json::json!({
            "type": "custom-title",
            "sessionId": empty_id,
            "customTitle": "New session",
            "mobileEmptySession": 1,
            "cwd": cwd.path().to_string_lossy(),
        });
        let empty_header = fixed_catalog_header(empty_id);
        std::fs::write(&empty_path, format!("{empty_header}\n{empty_line}\n"))
            .expect("write empty anchor");

        let json = list_sessions_json_from(cwd.path(), home.path(), None)
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
                "model": "deepseek-flash",
                "content": [{"type": "text", "text": "off branch"}]
            },
        });
        let header = fixed_catalog_header(session_id);
        std::fs::write(
            &transcript,
            format!("{header}\n{user}\n{assistant}\n{newer_sidechain}\n"),
        )
        .expect("write transcript");

        let json = list_sessions_json_from(cwd.path(), home.path(), None)
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

    #[tokio::test]
    async fn list_mode_narrowed_to_one_session_returns_its_catalog_row() {
        let cwd = tempfile::tempdir().expect("cwd tempdir");
        let home = tempfile::tempdir().expect("config tempdir");
        let ids = [
            "11111111-2222-4333-8444-555555555555",
            "66666666-7777-4888-8999-aaaaaaaaaaaa",
        ];
        for (index, id) in ids.iter().enumerate() {
            let path = session::jsonl::session_path(home.path(), &cwd.path().to_string_lossy(), id);
            std::fs::create_dir_all(path.parent().expect("catalog dir")).expect("catalog dir");
            let line = serde_json::json!({
                "type": "user",
                "uuid": format!("aaaaaaaa-bbbb-4ccc-8ddd-00000000000{index}"),
                "parentUuid": null,
                "sessionId": id,
                "timestamp": format!("2026-08-26T00:00:0{index}.000Z"),
                "cwd": cwd.path().to_string_lossy(),
                "isSidechain": false,
                "message": {"role": "user", "content": format!("prompt {index}")},
            });
            let header = fixed_catalog_header(id);
            std::fs::write(&path, format!("{header}\n{line}\n")).expect("write transcript");
        }

        let full: serde_json::Value = serde_json::from_str(
            &list_sessions_json_from(cwd.path(), home.path(), None)
                .await
                .expect("full catalog"),
        )
        .expect("valid envelope");
        for id in ids {
            let narrowed: serde_json::Value = serde_json::from_str(
                &list_sessions_json_from(cwd.path(), home.path(), Some(id))
                    .await
                    .expect("narrowed catalog"),
            )
            .expect("valid envelope");
            let expected = full["sessions"]
                .as_array()
                .and_then(|rows| rows.iter().find(|row| row["uuid"] == id))
                .expect("listed in the full catalog");
            assert_eq!(
                narrowed,
                serde_json::json!({"version": 1, "sessions": [expected]})
            );
        }

        let missing = list_sessions_json_from(
            cwd.path(),
            home.path(),
            Some("99999999-2222-4333-8444-555555555555"),
        )
        .await
        .expect("a foreign id is an empty catalog");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&missing).expect("valid envelope"),
            serde_json::json!({"version": 1, "sessions": []})
        );
        assert!(
            list_sessions_json_from(cwd.path(), home.path(), Some("not-a-uuid"))
                .await
                .is_err()
        );
    }

    /// Registering a session whose transcript already exists is allowed, and the
    /// guard's drop deregisters it.
    ///
    /// This used to also enqueue accepted and held work and assert both spilled
    /// on shutdown. That fixture raced `take_accepted_peer_reminders`, which
    /// drains the same queues from every turn's reminder assembly — another
    /// crate, no test lock — so the assertion failed on an empty spill. The
    /// spill is the global's own property and is pinned where the global lives,
    /// by `uds_inbox`'s `shutdown_spills_both_accepted_and_held_work_to_the_\
    /// bound_session`; that the bridge's drop is what triggers it is pinned by
    /// this module's `stale_live_session_guard_…`, which asserts the live guard
    /// stops the inbox.
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

        let mut cfg = resolve_desktop_config(&BridgeArgs::default()).unwrap();
        cfg.cwd = cwd.path().to_path_buf();
        cfg.lingxi_home = home.path().to_path_buf();
        cfg.session_id_override = Some(session_id.to_string());

        let guard =
            initialize_live_session(&mut cfg, None).expect("transcript is not a live writer");
        assert_eq!(guard.session_id, session_id);
        assert!(guard
            .dir
            .list_live()
            .unwrap()
            .iter()
            .any(|record| record.sid() == session_id));
        drop(guard);
        let dir = LiveSessionDir::at_live(home.path().join("sessions"));
        assert!(dir.list_live().unwrap().is_empty());
    }

    /// A stale generation's guard must not stop the inbox, unregister the live
    /// generation, or disturb the retired session's inbox.
    ///
    /// Nothing here depends on state that `take_accepted_peer_reminders` can
    /// drain. That one function empties BOTH stores this test used to rely on —
    /// the process-global accepted queue AND
    /// `process_live_dir().drain_inbox(process_session_id())` — and every turn
    /// calls it while assembling reminders, from another crate, holding no test
    /// lock. A serial lock between tests cannot exclude it. So the fixture uses
    /// `send_inbox` rather than the queue, and seeds it only once the process
    /// session id has moved to C, which puts A's file out of that drainer's
    /// reach. The spill those steps used to exercise is not this test's subject
    /// and is covered where the global lives, by `uds_inbox`'s own
    /// `retarget_spills_old_generation_work_to_old_session` and its shutdown
    /// siblings, under that crate's `test_guard`.
    #[test]
    fn stale_live_session_guard_cannot_stop_or_unregister_new_generation() {
        let _serial = crate::driver::LOOP_KA_TEST_SERIAL
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let cwd = tempfile::tempdir().expect("cwd tempdir");
        let home = tempfile::tempdir().expect("config tempdir");
        let session_a = "aaaaaaaa-2222-4333-8444-555555555555";
        let session_c = "cccccccc-2222-4333-8444-555555555555";

        let mut cfg_a = resolve_desktop_config(&BridgeArgs::default()).unwrap();
        cfg_a.cwd = cwd.path().to_path_buf();
        cfg_a.lingxi_home = home.path().to_path_buf();
        cfg_a.session_id_override = Some(session_a.to_string());
        let guard_a = initialize_live_session(&mut cfg_a, None).expect("start generation A");

        let mut cfg_c = resolve_desktop_config(&BridgeArgs::default()).unwrap();
        cfg_c.cwd = cwd.path().to_path_buf();
        cfg_c.lingxi_home = home.path().to_path_buf();
        cfg_c.session_id_override = Some(session_c.to_string());
        let guard_c = initialize_live_session(&mut cfg_c, None).expect("start generation C");
        let dir = guard_c.dir.clone();

        // Seed A's inbox only AFTER C has taken the process session id. While A
        // is still the live generation its file inbox is fair game for
        // `live_sessions::take_accepted_peer_reminders`, which drains
        // `process_live_dir().drain_inbox(process_session_id())` — and that is
        // called by every turn, from another crate, holding no test lock. Seeded
        // before C, this file was deleted out from under the assertion below.
        let mut owned_by_a = lingxi_core::host::live_sessions::outbound_peer_message(
            "sender",
            "source-session",
            &"owned by A".into(),
            None,
        );
        owned_by_a.msg_id = Some("accepted-a".into());
        dir.send_inbox(session_a, &owned_by_a)
            .expect("seed the retired session inbox");
        let a_inbox_path = dir.root().join(format!("{session_a}.inbox.jsonl"));
        let a_inbox_before = std::fs::read(&a_inbox_path).expect("fixture inbox exists");

        drop(guard_a);

        assert_eq!(
            lingxi_core::host::live_sessions::process_session_id().as_deref(),
            Some(session_c),
            "a stale guard must not take the process session id from C"
        );
        assert!(
            lingxi_core::host::uds_inbox::process_socket_path().is_some(),
            "a stale guard must not stop C's inbox"
        );
        assert!(
            dir.list_live()
                .unwrap()
                .iter()
                .any(|record| record.pid == std::process::id() && record.sid() == session_c),
            "a stale guard must not unregister C"
        );
        assert_eq!(
            std::fs::read(&a_inbox_path).expect("stale guard preserves A inbox"),
            a_inbox_before,
            "a stale guard must not rewrite the retired session's inbox"
        );

        // The LIVE guard still deregisters, and still leaves the retired inbox
        // alone — so the check above is about staleness, not about drops being
        // inert in general.
        drop(guard_c);

        assert!(dir.list_live().unwrap().is_empty());
        assert!(
            lingxi_core::host::uds_inbox::process_socket_path().is_none(),
            "the live guard DOES stop the inbox — otherwise the stale-guard              assertion above would hold for a drop that never tears anything down"
        );
        assert_eq!(
            std::fs::read(&a_inbox_path).expect("shutdown preserves A inbox"),
            a_inbox_before
        );
        assert_eq!(
            dir.drain_inbox(session_a).unwrap()[0].msg_id.as_deref(),
            Some("accepted-a")
        );
    }

    #[test]
    fn parse_help_flag() {
        assert!(BridgeArgs::parse(["--help"]).unwrap().help);
        assert!(BridgeArgs::parse(["-h"]).unwrap().help);
    }

    #[test]
    fn parse_missing_value_is_error() {
        assert!(BridgeArgs::parse(["--app-version"]).is_err());
        assert!(BridgeArgs::parse(["--cwd"]).is_err());
        assert!(BridgeArgs::parse(["--model"]).is_err());
        assert!(BridgeArgs::parse(["--bridge-dir"]).is_err());
    }

    #[test]
    fn parse_desktop_app_version_without_using_the_bridge_package_version() {
        assert_eq!(
            BridgeArgs::parse(["--app-version", "0.1.0+desktop.7"])
                .unwrap()
                .app_version
                .as_deref(),
            Some("0.1.0+desktop.7")
        );
    }

    #[test]
    fn parse_private_insights_policy_verdict_without_public_help_switch() {
        assert_eq!(
            BridgeArgs::parse(["--host-insights-policy-gate", "true"])
                .unwrap()
                .insights_policy_gate,
            Some(true)
        );
        assert_eq!(
            BridgeArgs::parse(["--host-insights-policy-gate", "false"])
                .unwrap()
                .insights_policy_gate,
            Some(false)
        );
        assert!(BridgeArgs::parse(["--host-insights-policy-gate", "yes"]).is_err());
        assert!(!usage().contains("--host-insights-policy-gate"));
    }

    #[test]
    fn parse_private_team_onboarding_policy_verdict_without_public_help_switch() {
        assert_eq!(
            BridgeArgs::parse(["--host-team-onboarding-policy-gate", "true"])
                .unwrap()
                .team_onboarding_policy_gate,
            Some(true)
        );
        assert_eq!(
            BridgeArgs::parse(["--host-team-onboarding-policy-gate", "false"])
                .unwrap()
                .team_onboarding_policy_gate,
            Some(false)
        );
        assert!(BridgeArgs::parse(["--host-team-onboarding-policy-gate", "yes"]).is_err());
        assert!(!usage().contains("--host-team-onboarding-policy-gate"));
    }

    #[test]
    fn only_explicit_scope_controller_enables_automation_dispatch() {
        let ordinary = BridgeArgs::parse(["--trusted-workspace"]).unwrap();
        assert!(!ordinary.scheduled_controller);
        assert!(
            !resolve_desktop_config(&ordinary)
                .unwrap()
                .enable_automation_scheduler
        );
        let controller =
            BridgeArgs::parse(["--trusted-workspace", "--scheduled-controller"]).unwrap();
        assert!(controller.scheduled_controller);
        assert!(
            resolve_desktop_config(&controller)
                .unwrap()
                .enable_automation_scheduler
        );
        assert!(usage().contains("--scheduled-controller"));
    }

    #[test]
    fn parse_security_flags_and_absolute_bridge_dir() {
        let args = BridgeArgs::parse([
            "--credential-stdin",
            "--trusted-workspace",
            "--packaged-credential-stdin-only",
            "--bridge-dir",
            "/tmp/lingxi-bridge",
        ])
        .expect("security flags parse");
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
        assert!(u.contains("--credential-stdin"));
        assert!(!u.contains("ANTHROPIC_API_KEY"));
        // Match a key-shaped token, not the substring in --ask-user-question-timeout.
        assert!(
            !u.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-' && ch != '_')
                .any(|token| token.starts_with("sk-")),
            "usage must not embed a key literal"
        );
    }

    #[test]
    fn credential_envelope_rejects_removed_provider_slots() {
        for field in ["api_key", "provider_keys", "openai_oauth"] {
            let bytes = serde_json::to_vec(&serde_json::json!({field: null})).unwrap();
            let error = read_credential_envelope(&mut std::io::Cursor::new(bytes)).unwrap_err();
            assert_eq!(error, "invalid credential envelope");
        }
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
    fn deferred_account_envelope_retains_host_domain_without_credentials() {
        let mut input = std::io::Cursor::new(
            br#"{"provider_account_home":"/host/accounts","provider_account_backend":"desktop-broker:development","plugin_secrets":{}}"#.to_vec(),
        );
        let envelope = read_credential_envelope(&mut input).unwrap();
        assert_eq!(
            envelope.provider_account_home,
            Some(PathBuf::from("/host/accounts"))
        );
        assert_eq!(
            envelope.provider_account_backend.as_deref(),
            Some("desktop-broker:development")
        );
        assert!(envelope.provider_accounts.is_none());
        assert!(envelope.provider_account_credentials.is_empty());
    }

    #[test]
    fn deferred_account_scope_matches_later_broker_projection_in_both_channels() {
        let root = tempfile::tempdir().unwrap();
        let mut scopes = Vec::new();
        for backend in ["desktop-broker:development", "desktop-broker:production"] {
            let projected =
                harness_runtime::provider_accounts::account_store_scope(root.path(), backend);
            let initial =
                resolve_provider_account_scope(root.path(), None, Some(backend), "Memory").unwrap();
            assert_eq!(initial, projected);
            assert_eq!(
                resolve_provider_account_scope(
                    root.path(),
                    Some(&projected),
                    Some(backend),
                    "Native"
                )
                .unwrap(),
                initial
            );
            assert!(resolve_provider_account_scope(
                root.path(),
                Some("another-domain"),
                Some(backend),
                "Memory"
            )
            .is_err());
            scopes.push(initial);
        }
        assert_ne!(scopes[0], scopes[1]);
        assert!(resolve_provider_account_scope(
            root.path(),
            None,
            Some("desktop-broker:other"),
            "Memory"
        )
        .is_err());
        assert_eq!(
            resolve_provider_account_scope(root.path(), Some("legacy-domain"), None, "Native")
                .unwrap(),
            "legacy-domain"
        );
    }

    #[test]
    fn resolve_uses_model_override() {
        let args = BridgeArgs {
            model: Some("custom-model".to_string()),
            ..BridgeArgs::default()
        };
        let cfg = resolve_desktop_config(&args).unwrap();
        assert_eq!(cfg.default_model, "custom-model");
        // A transport always binds the adapter gate.
        assert!(!cfg.use_noop_permission_gate);
    }

    #[test]
    fn workspace_is_untrusted_by_default() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("settings.json"), r#"{"providers":{"test":{"type":"openai"}},"routing":{"aliases":{"alias":"test/model"}}}"#).unwrap();
        let cfg = resolve_desktop_config(&BridgeArgs {
            session_config_home: Some(home.path().to_path_buf()),
            ..BridgeArgs::default()
        })
        .unwrap();
        assert_eq!(cfg.host_workspace_trusted, Some(false));
        assert_eq!(
            cfg.mod_render_surface,
            Some(harness_runtime::desktop::ModRenderSurface::Desktop)
        );
        assert!(cfg.provider_profiles.as_ref().unwrap().contains_key("test"));
        assert!(cfg.routing.is_some());
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
        })
        .unwrap();
        assert_eq!(cfg.host_workspace_trusted, Some(true));
        assert_eq!(cfg.permission_mode, permission::PermissionMode::Auto);
        assert!(!cfg.permission_mode_cli_explicit);
        assert_eq!(cfg.setting_source_scope, (true, true));
        assert_eq!(
            cfg.customization_gates,
            harness_runtime::desktop::CustomizationGates::default()
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
        let (parsed, warnings) = llm_runtime::parse_provider_profiles_lenient(
            &filtered,
            llm_runtime::ProviderParseOptions::lenient_deferred_static(),
        );
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
        })
        .unwrap();
        assert_eq!(cfg.permission_mode, permission::PermissionMode::Auto);
        assert!(!cfg.permission_mode_cli_explicit);
        assert_eq!(cfg.setting_source_scope, (true, true));
        assert_eq!(
            cfg.customization_gates,
            harness_runtime::desktop::CustomizationGates::default()
        );
        assert_eq!(cfg.mcp_paths.len(), 2);
        assert!(cfg.memory_provider.is_some());
        assert!(cfg.provider_profiles.as_ref().is_none_or(|profiles| {
            profiles
                .values()
                .all(|profile| profile.get("apiKeyEnv").is_none())
        }));
        assert_eq!(
            cfg.credential_storage_policy,
            CredentialStoragePolicy::NativeOrMemory,
            "packaged bridge credentials must remain process-local"
        );
    }

    // Debug builds retain larger execution futures than optimized Desktop
    // builds. Give both the polling thread and spawned workers a test budget.
    fn run_runtime_test<F, Fut>(test: F)
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()>,
    {
        let worker = std::thread::Builder::new()
            .name("desktop-runtime-test".into())
            .stack_size(64 * 1024 * 1024)
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .worker_threads(2)
                    .thread_stack_size(64 * 1024 * 1024)
                    .build()
                    .expect("build debug test Tokio runtime");
                runtime.block_on(test());
            })
            .expect("spawn debug runtime test thread");
        if let Err(panic) = worker.join() {
            std::panic::resume_unwind(panic);
        }
    }

    /// Drive a local injector through the production-assembled router and an
    /// authenticated Bridge connection. It must become an owned turn rather
    /// than display-only output, regardless of the user's `/goal` policy.
    #[test]
    fn assembled_bridge_runs_injected_local_command_as_owned_turn() {
        run_runtime_test(|| async {
            use bridge::wire::Frame;
            use bridge::{BridgeRequest, Capabilities, ClientHello, BRIDGE_PROTOCOL_VERSION};
            use client::protocol::{commands::ClientCommand, events::ClientEvent};
            use command_api::model::{BuiltinCommandHandler, CommandResult};
            use futures_util::{SinkExt, StreamExt};
            use std::sync::atomic::{AtomicBool, Ordering};
            use std::time::Duration;
            use tokio_tungstenite::tungstenite::handshake::client::generate_key;
            use tokio_tungstenite::tungstenite::Message;

            struct InjectFixture(Arc<AtomicBool>);
            #[async_trait::async_trait]
            impl BuiltinCommandHandler for InjectFixture {
                async fn handle(
                    &self,
                    _: &command_api::parser::ParsedSlashCommand,
                ) -> CommandResult {
                    self.0.store(true, Ordering::SeqCst);
                    CommandResult::InjectMessage {
                        content: "test-only injected prompt".into(),
                    }
                }

                fn name(&self) -> &str {
                    "fixture-inject"
                }

                fn description(&self) -> &str {
                    "Deterministic local injector"
                }
            }

            let _loop_state = crate::driver::LOOP_KA_TEST_SERIAL
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let tmp = tempfile::tempdir().expect("tempdir");
            let mut cfg = DesktopConfig::default();
            cfg.cwd = tmp.path().to_path_buf();
            cfg.lingxi_home = tmp.path().join(".lingxi");
            cfg.host_workspace_trusted = Some(true);
            cfg.restricted = true;
            cfg.isolated_credential_storage = true;
            cfg.default_model_explicit = true;
            cfg.use_noop_permission_gate = false;
            cfg.permission_mode = permission::PermissionMode::Default;
            cfg.enable_automation_scheduler = false;
            cfg.mod_render_surface = Some(harness_runtime::desktop::ModRenderSurface::Desktop);
            let bound = assemble(cfg).await.expect("assemble must succeed");
            let called = Arc::new(AtomicBool::new(false));
            bound
                .runtime
                .shared_command_registry
                .write()
                .await
                .register_builtin_handler(Arc::new(InjectFixture(called.clone())));
            let lifecycle = bound.session_lifecycle();
            let endpoint =
                McpEndpoint::start_on_ephemeral_port_with_pump(Arc::new(bound.connection))
                    .await
                    .expect("endpoint must start");
            const TOKEN: &str = "inject-test-token-32chars00000000";
            endpoint.set_auth_token(TOKEN.to_string());
            let port = endpoint.port();
            let request = http::Request::builder()
                .method("GET")
                .uri(format!("ws://127.0.0.1:{port}/mcp"))
                .header("host", format!("127.0.0.1:{port}"))
                .header("connection", "upgrade")
                .header("upgrade", "websocket")
                .header("sec-websocket-version", "13")
                .header("sec-websocket-key", generate_key())
                .header("sec-websocket-protocol", "mcp")
                .header("x-lingxi-ide-authorization", TOKEN)
                .body(())
                .unwrap();
            let (mut ws, response) = tokio_tungstenite::connect_async(request)
                .await
                .expect("WebSocket upgrade");
            assert_eq!(response.status(), 101);
            let frame = |id, method: &str, value: serde_json::Value| {
                Frame::Request(BridgeRequest {
                    trace_context: None,
                    id,
                    method: method.into(),
                    params: lingxi_core::types::utf16_json::Utf16JsonProjection::parse(
                        &value.to_string(),
                    )
                    .unwrap(),
                })
            };
            let hello = frame(
                1,
                "hello",
                serde_json::to_value(ClientHello {
                    protocol_version: BRIDGE_PROTOCOL_VERSION.to_string(),
                    client_name: "inject-test".into(),
                    capabilities: Capabilities::default(),
                })
                .unwrap(),
            );
            ws.send(Message::Text(serde_json::to_string(&hello).unwrap()))
                .await
                .unwrap();
            let response = tokio::time::timeout(Duration::from_secs(10), ws.next())
                .await
                .expect("hello deadline")
                .expect("hello response")
                .expect("hello WebSocket message");
            let Message::Text(response) = response else {
                panic!("hello must return a text response");
            };
            assert!(matches!(
                serde_json::from_str::<Frame>(&response).unwrap(),
                Frame::Response(response) if response.id == 1 && response.error.is_none()
            ));

            let submit = frame(
                2,
                "submit",
                serde_json::to_value(ClientCommand::RunSlashCommand {
                    raw: "/fixture-inject".into(),
                    images: Vec::new(),
                    turn_id: Some(47),
                })
                .unwrap(),
            );
            ws.send(Message::Text(serde_json::to_string(&submit).unwrap()))
                .await
                .unwrap();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
            let mut started = false;
            loop {
                let message = tokio::time::timeout_at(deadline, ws.next())
                    .await
                    .expect("injected command must reach a terminal turn")
                    .expect("WebSocket remains open")
                    .expect("WebSocket message");
                let Message::Text(message) = message else {
                    continue;
                };
                match serde_json::from_str::<Frame>(&message).unwrap() {
                    Frame::Event(ClientEvent::TurnStarted { turn_id }) => {
                        assert_eq!(turn_id, Some(47));
                        assert!(!started, "command started more than one turn");
                        assert!(called.load(Ordering::SeqCst), "fixture handler must run");
                        started = true;
                    }
                    Frame::Event(ClientEvent::TurnEnded { .. }) => {
                        assert!(started, "injected prompt must start a turn before ending");
                        break;
                    }
                    Frame::Event(ClientEvent::SlashCommandResult { display, .. }) => {
                        panic!("injected prompt must not be display-only: {display:?}");
                    }
                    _ => {}
                }
            }
            ws.close(None).await.unwrap();
            endpoint.shutdown().await;
            let report = lifecycle.shutdown_and_drain().await;
            assert!(report.complete, "shutdown errors: {:?}", report.errors);
        });
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
        // Allow this one test dispatch through the real policy gate without a
        // provider-backed Auto-mode classifier or an interactive prompt.
        let settings_dir = cwd.join(".lingxi");
        std::fs::create_dir_all(&settings_dir).expect("create isolated settings directory");
        std::fs::write(
            settings_dir.join("settings.local.json"),
            r#"{"permissions":{"defaultMode":"default","allow":["media_status"]}}"#,
        )
        .expect("configure the media_status allow rule");
        let cfg = {
            let mut __desktop_config = DesktopConfig::default();
            __desktop_config.remote_workspace = false;
            __desktop_config.diskless = false;
            __desktop_config.trusted_network_directories = Vec::new();
            __desktop_config.flag_settings_source = None;
            __desktop_config.diagnostics = None;
            __desktop_config.composition = None;
            __desktop_config.defer_session_start = false;
            __desktop_config.connect_mcp_in_background = false;
            __desktop_config.session_transcript_path = None;
            __desktop_config.session_transcript_authority = None;
            __desktop_config.session_resume_snapshot = None;
            __desktop_config.session_resume_cost = None;
            __desktop_config.max_structured_output_retries = 5;
            __desktop_config.mcp_services_factory = None;
            __desktop_config.user_agent_environment = None;
            __desktop_config.request_identity = None;
            __desktop_config.anthropic_compatible_version = None;
            __desktop_config.anthropic_client_metadata = None;
            __desktop_config.native_thinking_display = None;
            __desktop_config.verified_computer_profiles = Vec::new();
            __desktop_config.build_info = harness_runtime::desktop::BuildInfo::new(
                env!("CARGO_PKG_VERSION"),
                option_env!("LINGXI_GIT_SHA_SHORT").unwrap_or("unknown"),
            );
            __desktop_config.enable_automation_scheduler = false;
            __desktop_config.host_workspace_trusted = Some(true);
            __desktop_config.insights_policy_gate = None;
            __desktop_config.mod_render_surface =
                Some(harness_runtime::desktop::ModRenderSurface::Desktop);
            __desktop_config.native_settings_agent_context =
                lingxi_core::host::NativeSettingsAgentContext::Main;
            __desktop_config.initial_teammate_team_name = None;
            // This unit test must not require the signed macOS Credential
            // Broker or inherit a developer login keychain.
            __desktop_config.isolated_credential_storage = true;
            __desktop_config.credential_storage_policy = CredentialStoragePolicy::NativePreferred;
            __desktop_config.api_base = DEFAULT_API_BASE.to_string();
            // (M13) Inert auth-resolver inputs: no managed OAuth forcing, no
            // FD-inherited key.
            __desktop_config.cwd = cwd.clone();
            __desktop_config.lingxi_home = settings_dir;
            __desktop_config.default_model = "claude-sonnet-4-20250514".to_string();
            // Deterministic across host machines: a dev keychain with real
            // provider keys must not trigger the connected-provider fallback.
            __desktop_config.default_model_explicit = true;
            __desktop_config.recent_models = Vec::new();
            __desktop_config.fallback_model = None;
            __desktop_config.custom_betas = Vec::new();
            __desktop_config.flag_settings = None;
            __desktop_config.provider_profiles = None;
            __desktop_config.routing = None;
            __desktop_config.mcp_paths = vec![cwd.join(".mcp.json")];
            __desktop_config.use_noop_permission_gate = false;
            __desktop_config.deny_unresolved_ask = false;
            __desktop_config.is_tty = false;
            __desktop_config.injected_permission_gate = None;
            __desktop_config.injected_plugin_secrets = BTreeMap::new();
            __desktop_config.ask_user_question_tx = None;
            __desktop_config.computer_access_tx = None;
            __desktop_config.session_agent_observer = None;
            __desktop_config.session_started_as_coordinator = false;
            // Deterministic test: empty memory, never the real FS.
            __desktop_config.memory_provider = None;
            __desktop_config.permission_mode = permission::PermissionMode::Default;
            __desktop_config.permission_mode_cli = None;
            __desktop_config.permission_mode_preference = None;
            __desktop_config.permission_mode_cli_explicit = false;
            __desktop_config.allow_dangerously_skip_permissions = false;
            __desktop_config.max_turns = None;
            __desktop_config.plan_mode_instructions = None;
            __desktop_config.plans_directory = None;
            __desktop_config.max_budget_usd = None;
            __desktop_config.json_schema = None;
            __desktop_config.system_prompt_override = None;
            __desktop_config.append_system_prompt = None;
            __desktop_config.session_id_override = None;
            __desktop_config.session_writer_lease = None;
            __desktop_config.parent_session_id = None;
            __desktop_config.disable_slash_commands = false;
            __desktop_config.session_skill_allowlist = None;
            __desktop_config.add_dir = Vec::new();
            __desktop_config.cli_mcp_servers = Vec::new();
            __desktop_config.strict_mcp_config = false;
            __desktop_config.restricted = false;
            __desktop_config.restricted_tools = None;
            __desktop_config.exclude_dynamic_system_prompt_sections = false;
            __desktop_config.setting_source_scope = (true, true);
            __desktop_config.customization_gates =
                harness_runtime::desktop::CustomizationGates::default();
            __desktop_config.session_persistence = true;
            __desktop_config.cli_agents_json = None;
            __desktop_config.cli_agent = None;
            __desktop_config.cli_plugin_dirs = Vec::new();
            __desktop_config.initial_effort = None;
            __desktop_config.default_model_env_pinned = false;
            __desktop_config.session_thinking = Default::default();
            __desktop_config.bg_session_forker = None;
            __desktop_config.worktree_launch = None;
            __desktop_config.tmux_launch = None;
            // `assemble` supplies the routed audio service for this connection.
            __desktop_config.audio = None;
            __desktop_config.inline_visualization = false;
            __desktop_config.local_apps = false;
            __desktop_config.private_pages = false;
            __desktop_config
        };
        let bound = assemble(cfg).await.expect("assemble must succeed");
        assert!(
            bound.runtime.has_audio(),
            "assembly injects audio into the runtime"
        );
        assert!(
            Arc::ptr_eq(
                &bound
                    .runtime
                    .media_service()
                    .expect("runtime media service"),
                &bound.media_host.media_service(),
            ),
            "assembly injects the same app-scoped media facade into the runtime"
        );
        // Exercise the actual assembled tool path. A missing typed media
        // asset produces a service-owned not-found result without requiring
        // a device or a connection to answer a native request.
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            Box::pin(
                bound.runtime.orchestrator.call_tool_from_host(
                    lingxi_core::types::ToolUseId::new(),
                    "media_status".into(),
                    lingxi_core::types::utf16_json::Utf16JsonProjection::parse(
                        r#"{"target":{"kind":"media","ref":"missing-fixture-asset"}}"#,
                    )
                    .unwrap(),
                    None,
                ),
            ),
        )
        .await
        .expect("assembled media dispatch must finish")
        .expect("assembled media dispatch must complete")
        .expect("media_status must be dispatchable");
        match result {
            lingxi_core::types::ContentBlock::ToolResult {
                is_error, content, ..
            } => {
                assert_eq!(is_error, Some(true));
                assert!(
                    content
                        .to_json_string()
                        .unwrap()
                        .contains("media asset was not found"),
                    "the assembled media service must report its missing asset: {content:?}"
                );
            }
            other => panic!("expected a media tool result, got {other:?}"),
        }
        assert!(bound.runtime.orchestrator.workspace_trusted().await);
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

        let mut requested_exit = bound.wait_for_exit_requested();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), &mut requested_exit)
                .await
                .is_err(),
            "an idle session must not request process exit"
        );
        bound.runtime.orchestrator.request_exit().await;
        let refusal = tokio::time::timeout(std::time::Duration::from_secs(1), requested_exit)
            .await
            .expect("authenticated RequestExit must wake host teardown");
        assert!(
            refusal.is_none(),
            "ordinary RequestExit is not a policy refusal"
        );
        close_unserved_for_test(&bound).await;
        let report = bound.session_lifecycle().shutdown_and_drain().await;
        assert!(report.complete, "shutdown errors: {:?}", report.errors);
    }
}
