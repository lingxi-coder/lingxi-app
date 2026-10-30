//! F2-08 — Full command/event routing.
//!
//! The walking skeleton ([`crate::server::BridgeConnection`], F2-05/F2-06)
//! routes only the turn + permission path. This module is the engine-routing
//! seam for the FULL [`ClientCommand`] surface: model switch + listings + slash
//! commands + session control + the background-task poll loop.
//!
//! ## The routing seam (mirrors `TurnDriver`)
//!
//! [`CommandRouter`] is the abstraction the connection delegates every
//! non-turn/non-permission command to — exactly the way
//! [`crate::server::TurnDriver`] abstracts the turn entry. The production server
//! binds an [`EngineCommandRouter`] wrapping the real engine handles
//! ([`lingxi_core::host::OrchestratorHandle`], [`lingxi_core::host::AuthHandle`],
//! [`lingxi_core::host::task_registry::TaskRegistryHandle`], and the slash dispatcher); a
//! test binds the SAME router over the engine's `MockOrchestratorHandle` / mock
//! task + auth handles, so the routing-and-lowering path under test is the
//! production one (no test-only router shim).
//!
//! Each command maps to its engine entry, and the engine's reply is lowered to
//! a [`ClientEvent`] (via the pure `client::adapter::lowering` fns — the single
//! parity surface, §0.2) and pushed out through the connection's
//! [`ClientEventSink`]:
//!
//! | command | engine entry | reply event |
//! |---|---|---|
//! | `SetModel` | `OrchestratorHandle::switch_model` | `ModelChanged` |
//! | `SetPermissionMode` | `OrchestratorHandle::set_permission_mode` | `PermissionModeChanged` |
//! | `ListModels` / `RefreshListings{Models}` | `list_available_models` + status | `ModelList` |
//! | `RefreshListings{Mcp}` | `list_mcp_servers` | `McpServers` |
//! | `RefreshListings{Hooks}` | `list_hooks` | `Hooks` |
//! | `RefreshListings{Agents}` | `list_agents` | `Agents` |
//! | `RefreshListings{Status}` | `get_status_snapshot` | `StatusSnapshot` |
//! | `RefreshListings{Doctor}` | `run_doctor_checks` | `DoctorReport` |
//! | `RefreshListings{Auth}` / `Login` / `Logout` | `AuthHandle::*` | `AuthState` |
//! | `RefreshListings{Tasks}` / `TaskList` | `TaskRegistryHandle::list` | `TaskRow`× |
//! | `RunSlashCommand` | `SlashCommandDispatcher::dispatch` | `SlashCommandResult` or normal turn stream |
//! | `TaskOutput` | `TaskRegistryHandle::output` | `TaskOutputChunk` |
//! | `TaskStop` | `TaskRegistryHandle::kill` | `TaskStatusChanged` |
//! | `ForceCompact` | `force_compact` | `CompactionCompleted` |
//! | `ClearSession` | `clear_session` (REJECTED mid-turn) | `SessionEnded` / `Error` |
//! | `ListSessions` / `RefreshListings{Sessions}` | persisted JSONL catalog | `SessionList` |
//! | `NewSession` | `clear_session` + optional `switch_model` | `SessionStarted` / `Error` |
//! | `ResumeSession` | JSONL replay + `resume_session` | `SessionResumed` / `Error` |
//! | `RefreshListings{Settings}` | `settings_bridge::build_snapshot` | `SettingsSnapshot` |
//! | `UpdatePermissionRules` | `permission::persist_permission_rule_set` | `SettingsSnapshot` / `Error` |
//! | `SetDefaultPermissionMode` | `permission::persist_permission_mode` | `SettingsSnapshot` / `Error` |
//! | `UpdateWorkspaceDirectories` | `permission::persist_workspace_directories` | `SettingsSnapshot` / `Error` |
//! | `RequestExit` | `request_exit` | — |
//!
//! ## Mid-turn semantics
//!
//! Session mutations are **rejected while a turn is in flight** (plan §2): the
//! connection sets [`EngineCommandRouter::set_turn_active`] on `SendPrompt` and
//! clears it on turn end; a clear/new/resume arriving in that window is refused
//! with a [`ClientEvent::Error`] and never reaches the engine/store.
//!
//! ## Reserved / feed-deferred
//!
//! Per governing decisions §0.7/§0.9, the router NEVER live-sources the reserved
//! DTOs (`ThinkingDelta`, `UsageUpdate`, `CoordinatorStatus`); the corresponding
//! engine sources do not exist in the foundation, so no command maps to them.
//! The listing kind with no engine handle or host store in the foundation
//! (`Memory`) remains unrouted here. `Settings` IS routed: it reads the layered
//! settings files through [`crate::settings_bridge`] using the
//! [`crate::settings_bridge::SettingsContext`] the composition root supplies.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use audio_provider::{media_cloud::AudioProviderCloudRegistry, AudioProviderHost};
pub(crate) use client::adapter::controls::decode_reasoning_selection;
use client::adapter::lowering::{
    lower_provider_model_catalog_entry, lower_status_snapshot, lower_task_output_chunk,
};
use client::adapter::ClientEventSink;
use client::protocol::commands::{ClientCommand, ListingKindDto};
use client::protocol::events::{ClientEvent, ErrorKindDto};
use client::protocol::listings::{AuthStateDto, SessionModeDto};
use client::protocol::text::ClientTextDto;
use command_api::model::CommandSource;
use command_api::parser::parse_slash_command;
use command_api::registry::CommandRegistry;
use lingxi_core::host::auth::{AuthHandle, LoginInfo};
use lingxi_core::host::orchestrator::HandleError;
use lingxi_core::host::orchestrator::OrchestratorHandle;
use lingxi_core::host::task_registry::{TaskListFilter, TaskRegistryHandle};
use lingxi_core::host::SlashCommandDispatcher;
use lingxi_core::types::utf16_json::Utf16JsonProjection;
use media_api::MediaOwner;
use media_runtime::{MediaAttachmentHandle, MediaAttachmentPhase};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::mcp_bridge::McpPaths;
use crate::settings_bridge::SettingsContext;

/// Default number of recent sessions returned when `ListSessions` omits its
/// explicit limit. This matches the CLI `/resume` picker and the mobile host.
const DEFAULT_SESSION_LIST_LIMIT: usize = 5;

fn split_model_selection_source(
    source: Utf16JsonProjection,
    listings: &[lingxi_core::host::ModelListing],
) -> (
    Utf16JsonProjection,
    Option<lingxi_core::host::ModelRouteAnchor>,
) {
    let Some(units) = source.string_units("") else {
        return (source, None);
    };
    let Ok(text) = String::from_utf16(&units) else {
        return (source, None);
    };
    let trimmed = lingxi_core::host::effort::trim_js_whitespace(&text);
    let (model, profile) = lingxi_core::host::parse_model_ref(&trimmed, listings);
    let route_hint = profile.map(|profile| lingxi_core::host::ModelRouteAnchor {
        model: model.clone(),
        profile: Some(profile),
    });
    if route_hint.is_some() || model != trimmed {
        (Utf16JsonProjection::from(model), route_hint)
    } else {
        (source, route_hint)
    }
}

/// Inputs needed to enumerate and replay the persisted JSONL session store.
///
/// Kept optional on [`EngineCommandRouter`] so existing embedded/test callers
/// that only need model/auth/task routing remain source-compatible. Production
/// bridge boot always supplies this context.
pub struct SessionStoreContext {
    lingxi_home: PathBuf,
    session_cwd: String,
    runtime_cwd: Option<Arc<tool_api::SessionCwd>>,
    fs: Arc<dyn lingxi_core::host::FileSystem>,
    transcript_writer: Option<Arc<session::jsonl::JsonlWriter>>,
}

impl SessionStoreContext {
    pub(crate) fn lingxi_home(&self) -> &std::path::Path {
        &self.lingxi_home
    }
    /// Follow the same cwd cell that the running runtime publishes on a move.
    pub fn with_runtime_cwd(mut self, cwd: Arc<tool_api::SessionCwd>) -> Self {
        self.runtime_cwd = Some(cwd);
        self
    }

    fn current_cwd(&self) -> String {
        self.runtime_cwd.as_ref().map_or_else(
            || self.session_cwd.clone(),
            |cwd| cwd.cwd().to_string_lossy().into_owned(),
        )
    }

    fn main_transcript_path(&self, session_id: &str) -> PathBuf {
        self.transcript_writer
            .as_ref()
            .filter(|writer| {
                writer
                    .active_path()
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .and_then(|name| uuid::Uuid::parse_str(name).ok())
                    .zip(uuid::Uuid::parse_str(session_id).ok())
                    .is_some_and(|(source, requested)| source == requested)
            })
            .map_or_else(
                || {
                    orchestrator::transcript_paths::main_transcript_path(
                        &self.lingxi_home,
                        &self.current_cwd(),
                        session_id,
                    )
                },
                |writer| writer.active_path(),
            )
    }

    fn subagents_dir(&self, session_id: &str) -> PathBuf {
        self.main_transcript_path(session_id)
            .with_extension("")
            .join("subagents")
    }

    /// Retain the prepared session's authoritative writer for identity snapshots.
    pub fn with_transcript_writer(
        mut self,
        writer: Option<Arc<session::jsonl::JsonlWriter>>,
    ) -> Self {
        self.transcript_writer = writer;
        self
    }

    /// Retain a zero-message conversation that owns a durable scheduled task.
    async fn ensure_scheduled_chat(&self, session_id: &str) -> Result<(), String> {
        let session_uuid = lingxi_core::types::SessionId::parse_prefixed(session_id)
            .ok_or("Invalid scheduled task chat identity")?
            .as_uuid()
            .to_string();
        let session_id = session_uuid.as_str();
        let path = self.main_transcript_path(session_id);
        let reader = session::jsonl::reader::JsonlReader::new(path.clone(), self.fs.clone());
        let mut title = "Scheduled task".to_string();
        match reader.read_routed().await {
            Ok(loaded) => {
                if !loaded.messages_in_order.is_empty()
                    || loaded.mobile_empty_sessions.contains(session_id)
                {
                    return Ok(());
                }
                if loaded.malformed_line_count > 0 {
                    return Err("Cannot attach a scheduled task to an unreadable chat".into());
                }
                if let Some(existing) = loaded.custom_titles.get(session_id) {
                    title.clone_from(existing);
                }
            }
            Err(session::jsonl::reader::ReaderError::Fs(lingxi_core::host::FsError::NotFound(
                _,
            ))) => {}
            Err(error) => {
                // The desktop filesystem may wrap ENOENT as FsError::Io.
                // Verify absence without treating permission/corruption errors as empty.
                if !matches!(tokio::fs::try_exists(&path).await, Ok(false)) {
                    return Err(format!("Cannot read the scheduled task's chat: {error}"));
                }
            }
        }
        session::jsonl::writer::JsonlWriter::new(path, self.fs.clone())
            .append_mobile_empty_session(session_id, &title, &self.current_cwd())
            .await
            .map_err(|error| format!("Cannot retain the scheduled task's chat: {error}"))
    }

    /// Build a session-store context rooted at the desktop config directory and
    /// the connection's project cwd.
    #[must_use]
    pub fn new(
        lingxi_home: PathBuf,
        session_cwd: String,
        fs: Arc<dyn lingxi_core::host::FileSystem>,
    ) -> Self {
        Self {
            lingxi_home,
            session_cwd,
            runtime_cwd: None,
            fs,
            transcript_writer: None,
        }
    }
}

/// The engine-routing seam for the full [`ClientCommand`] surface (everything
/// except the turn + permission path the [`crate::server::BridgeConnection`]
/// handles directly).
///
/// Abstracting routing behind a trait keeps the connection loop decoupled from
/// HOW the engine was assembled: the production server builds an
/// [`EngineCommandRouter`] from `harness_runtime::desktop::build`'s `DesktopRuntime`
/// handles, while a routing test builds the SAME router over the engine's mock
/// handles.
#[async_trait]
pub trait CommandRouter: Send + Sync + 'static {
    /// Advertise realtime only when a concrete connection controller is installed.
    fn realtime_audio_supported(&self) -> bool {
        false
    }
    /// Reject new input immediately during connection/session teardown.
    fn abort_realtime_audio(&self) {}
    /// Wait for the native session to release tools and provider transport.
    async fn stop_realtime_audio(&self) {}
    /// Current provider profile, if configured for the authoritative session.
    async fn audio_session_context(&self) -> (Option<String>, Option<String>, Option<String>) {
        (None, None, None)
    }
    /// Bind provider routes to a replacement before it becomes visible.
    async fn prepare_media_attachment(
        &self,
        _attachment: &MediaAttachmentHandle,
        _owner: &MediaOwner,
    ) -> Result<(), String> {
        Ok(())
    }
    /// Drop provider binding metadata only after its attachment is confirmed.
    async fn cleanup_confirmed_media_binding(&self) {}
    /// Drain connection-owned in-memory child conversations before their event
    /// sink is released. Routers without such state need no close hook.
    async fn connection_closed(&self) {}
    /// Route one decoded [`ClientCommand`], emitting any reply event(s) through
    /// `sink`. Pure side-effecting: it never blocks the caller for a turn (the
    /// turn path lives on [`crate::server::TurnDriver`]).
    async fn route(&self, command: ClientCommand, sink: Arc<dyn ClientEventSink>);

    /// Product-only correlated read: existing protocol events form one atomic
    /// response rather than interleaving with live background status pushes.
    /// The caller fences outbound delivery through this read. Implementations
    /// must collect locally: never emit through the connection, mutate engine
    /// state, drain an interaction broker, or wait for the SDK turn gate.
    async fn runtime_snapshot(&self) -> Result<Vec<ClientEvent>, String> {
        Err("desktop runtime snapshot is unavailable".into())
    }

    /// Authenticated, private Native `set_cwd` control request.
    async fn set_working_directory(
        &self,
        _request: Utf16JsonProjection,
        _sink: Arc<dyn ClientEventSink>,
    ) -> Result<Utf16JsonProjection, String> {
        Err("set_cwd is unavailable".into())
    }

    async fn workspace_state(&self) -> Result<Utf16JsonProjection, String> {
        Err("workspace state is unavailable".into())
    }

    async fn restore_workspace_controls(
        &self,
        _request: Utf16JsonProjection,
        _sink: Arc<dyn ClientEventSink>,
    ) -> Result<Utf16JsonProjection, String> {
        Err("workspace controls are unavailable".into())
    }

    async fn add_working_directory(
        &self,
        _request: Utf16JsonProjection,
        _sink: Arc<dyn ClientEventSink>,
    ) -> Result<Utf16JsonProjection, String> {
        Err("add_directory is unavailable".into())
    }

    /// Resume the exact row selected by the Desktop's scoped metadata catalog.
    async fn resume_selected_session(
        &self,
        _request: Utf16JsonProjection,
        _sink: Arc<dyn ClientEventSink>,
    ) -> Result<Utf16JsonProjection, String> {
        Err("resume_selected_session is unavailable".into())
    }

    /// Dispatch a raw `/<name> [args]` line and hand back the dispatch RESULT so
    /// the connection (which owns the [`crate::server::TurnDriver`] + queue) can
    /// decide whether to PRINT it (display-only `type: "local"` commands) or run
    /// it AS a turn (`type: "prompt"` commands like `/loop`, plus Markdown /
    /// Plugin prompt commands — claude-code injects the expanded prompt as the
    /// user message). Returns `None` when no dispatcher is wired; callers must
    /// not retry expansion through the display-only `route` path.
    async fn dispatch_slash(
        &self,
        _raw: Utf16JsonProjection,
        _cancellation: CancellationToken,
    ) -> Option<SlashDispatchOutcome> {
        None
    }

    /// Whether `raw` resolves through this router's current slash registry to
    /// a built-in command that requires an idle session (session replacement
    /// or manual compaction). This is a
    /// side-effect-free admission query: callers use it only to refuse such a
    /// command while another turn owns the connection. Routers without a live
    /// command catalog leave the result unknown and preserve normal routing.
    async fn requires_idle_turn_for_slash(&self, _raw: &Utf16JsonProjection) -> bool {
        false
    }
}

pub(crate) fn slash_failure_display(diagnostic: &Utf16JsonProjection) -> Utf16JsonProjection {
    let units = diagnostic
        .string_units("/message")
        .expect("settings diagnostics carry their source message");
    Utf16JsonProjection::root_string(String::from_utf16_lossy(&units), units)
        .expect("display string matches its exact UTF-16 units")
}

#[cfg(test)]
mod slash_failure_display_tests {
    use super::*;

    #[test]
    fn slash_failure_display_keeps_the_exact_message_units() {
        let diagnostic = Utf16JsonProjection::parse(
            r#"{"kind":"settings_projection","message":"invalid\ud800","sourceValue":"other"}"#,
        )
        .unwrap();
        let display = slash_failure_display(&diagnostic);
        assert_eq!(
            display.string_units(""),
            Some(vec![
                b'i' as u16,
                b'n' as u16,
                b'v' as u16,
                b'a' as u16,
                b'l' as u16,
                b'i' as u16,
                b'd' as u16,
                0xd800,
            ])
        );
    }
}

#[cfg(test)]
mod session_transition_slash_tests {
    use super::*;

    #[test]
    fn session_transition_slashes_resolve_by_canonical_builtin_identity() {
        let mut registry = CommandRegistry::new();
        let mut clear = command_api::model::SlashCommand::default();
        clear.name = "clear".into();
        clear.source = CommandSource::Builtin;
        clear.aliases = vec!["reset".into(), "new".into()];
        registry.register_command(clear);
        let mut compact = command_api::model::SlashCommand::default();
        compact.name = "compact".into();
        compact.source = CommandSource::Builtin;
        registry.register_command(compact);
        let mut cd = command_api::model::SlashCommand::default();
        cd.name = "cd".into();
        cd.source = CommandSource::Builtin;
        registry.register_command(cd);

        for raw in ["/clear", "/new title", "/reset"] {
            assert!(
                requires_idle_turn_for_slash(&registry, &raw.into()),
                "{raw} should resolve to the session-clearing builtin"
            );
        }
        assert!(requires_idle_turn_for_slash(&registry, &"/compact".into()));
        assert!(requires_idle_turn_for_slash(
            &registry,
            &"/cd /tmp/path with spaces".into()
        ));

        // The parser and registry preserve case. Unknown spellings and custom
        // commands must not be reclassified as the builtin transition.
        assert!(!requires_idle_turn_for_slash(
            &registry,
            &"/NEW title".into()
        ));
        assert!(!requires_idle_turn_for_slash(&registry, &"/clearer".into()));

        let mut custom = command_api::model::SlashCommand::default();
        custom.name = "custom-clear".into();
        custom.source = CommandSource::Settings(lingxi_core::types::SettingsScope::Project);
        registry.register_command(custom);
        assert!(!requires_idle_turn_for_slash(
            &registry,
            &"/custom-clear".into()
        ));
    }

    #[test]
    fn session_store_uses_live_cwd_and_authoritative_writer_path() {
        let cwd = tool_api::SessionCwd::new(PathBuf::from("/old"), vec![PathBuf::from("/old")]);
        let fs: Arc<dyn lingxi_core::host::FileSystem> =
            Arc::new(crate::HostFileSystem::new("/old".into()));
        let id = "11111111-2222-4333-8444-555555abcdef";
        let source = PathBuf::from(format!(
            "/config/projects/source/{}.jsonl",
            id.to_uppercase()
        ));
        let writer = Arc::new(session::jsonl::JsonlWriter::new(source.clone(), fs.clone()));
        let store = SessionStoreContext::new("/config".into(), "/old".into(), fs)
            .with_runtime_cwd(cwd.clone())
            .with_transcript_writer(Some(writer));
        cwd.change_cwd("/new".into());
        assert_eq!(store.current_cwd(), "/new");
        assert_eq!(store.main_transcript_path(id), source);
        assert_eq!(
            store.subagents_dir(id),
            PathBuf::from(format!(
                "/config/projects/source/{}/subagents",
                id.to_uppercase()
            ))
        );
    }
}

pub(crate) fn source_text_eq_ascii(source: &Utf16JsonProjection, expected: &str) -> bool {
    source
        .string_units("")
        .is_some_and(|units| units.iter().copied().eq(expected.encode_utf16()))
}

pub(crate) fn source_text_eq_ascii_ignore_case(
    source: &Utf16JsonProjection,
    expected: &str,
) -> bool {
    source.string_units("").is_some_and(|units| {
        units.len() == expected.len()
            && units
                .iter()
                .copied()
                .zip(expected.bytes())
                .all(|(unit, expected)| {
                    unit <= 0x7f && (unit as u8).eq_ignore_ascii_case(&expected)
                })
    })
}

fn source_text_is_empty(source: &Utf16JsonProjection) -> bool {
    source
        .string_units("")
        .is_some_and(|units| units.is_empty())
}

fn concat_source_text(parts: &[&Utf16JsonProjection]) -> Utf16JsonProjection {
    let units = parts
        .iter()
        .flat_map(|part| {
            part.string_units("")
                .expect("slash prompt fragments are root strings")
        })
        .collect::<Vec<_>>();
    Utf16JsonProjection::root_string(String::from_utf16_lossy(&units), units)
        .expect("display projection is derived from the exact UTF-16 units")
}

#[derive(Default)]
struct RuntimeSnapshotEvents(std::sync::Mutex<Vec<ClientEvent>>);

#[async_trait]
impl ClientEventSink for RuntimeSnapshotEvents {
    async fn emit(&self, event: ClientEvent) {
        self.0
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(event);
    }
}

/// One already-dispatched slash result plus authoritative out-of-band changes
/// observed during that same dispatch. Keeping both together lets the bridge
/// connection route prompt commands without dispatching the command twice.
#[derive(Debug, Clone)]
pub struct SlashDispatchOutcome {
    /// The single engine dispatch result for the submitted command.
    pub result: lingxi_core::host::SlashDispatchResult,
    /// Full authoritative state/catalog events produced by that dispatch.
    pub authority_events: Vec<ClientEvent>,
}

/// Lower an `Option<LoginInfo>` to the auth-state DTO (`current_user` → wire).
fn lower_auth_state(info: Option<LoginInfo>) -> AuthStateDto {
    match info {
        Some(li) => AuthStateDto::SignedIn {
            email: li.email,
            org_id: li.org_id,
        },
        None => AuthStateDto::SignedOut,
    }
}

fn provider_model_catalog_from_listings(
    listings: &[lingxi_core::host::ModelListing],
) -> Vec<client::protocol::listings::ProviderModelCatalogEntryDto> {
    lingxi_core::host::provider_model_catalog(listings)
        .iter()
        .map(lower_provider_model_catalog_entry)
        .collect()
}

/// The production [`CommandRouter`]: wraps the real engine handles and lowers
/// each reply with the pure `client::adapter::lowering` parity fns.
pub struct EngineCommandRouter {
    workspace_commands: Option<Arc<crate::workspace_commands::WorkspaceCommands>>,
    session_cron: Option<Arc<cron::CronScheduler>>,
    cron_firer: Option<Arc<crate::cron_host::HostCronFirer>>,
    handle: Arc<dyn OrchestratorHandle>,
    auth: Arc<dyn AuthHandle>,
    tasks: Arc<dyn TaskRegistryHandle>,
    /// Optional slash dispatcher. The desktop runtime always supplies one; it is
    /// `Option` so a transport that has not yet built the command registry can
    /// still route the rest of the surface.
    dispatcher: Option<Arc<dyn SlashCommandDispatcher>>,
    /// Optional shared slash-command registry backing the dispatcher. When
    /// present it lets the router emit `SlashCommandCatalog` pulls and
    /// `CommandsChanged` pushes from the same live registry snapshot.
    slash_registry: Option<Arc<RwLock<CommandRegistry>>>,
    /// Ready parent and isolated recipe for RAM-only child conversations.
    side_chat_runtime: Option<(
        Arc<orchestrator::ConversationOrchestrator>,
        Arc<dyn harness_runtime::side_chat::SideChatFactory>,
    )>,
    /// One child manager per bridge connection generation.
    side_chat_manager: tokio::sync::Mutex<Option<Arc<harness_runtime::side_chat::SideChatManager>>>,
    /// Optional persisted-session catalog/replay context. Production boot wires
    /// it; lightweight users of the routing seam can omit it.
    session_store: Option<SessionStoreContext>,
    /// Process-owned observer facts supplement the live task registry for
    /// foreground children and allocation before the first transcript write.
    session_agent_observer:
        Option<Arc<harness_runtime::desktop::session_agents::DesktopSessionAgentObserver>>,
    /// Current coordinator ownership supplements persisted agent transcripts
    /// when a host reconnects after missing background completion events.
    team_registry: Option<Arc<dyn lingxi_core::host::team_registry::TeamRegistryHandle>>,
    /// Shared provider credential manager. Production bridge boot wires the
    /// exact manager used by the runtime; tests/embedded clients may omit it.
    catalog_registry: harness_runtime::desktop::FusionCatalogRegistry,
    credentials: Option<Arc<secret::CredentialManager>>,
    realtime_audio: Option<Arc<crate::realtime_audio::RealtimeAudioController>>,
    media_cloud_audio: Option<Arc<AudioProviderCloudRegistry>>,
    media_cloud_attachment_slot: Option<Arc<std::sync::RwLock<Option<MediaAttachmentHandle>>>>,
    media_cloud_audio_provider: Option<Arc<AudioProviderHost>>,
    media_cloud_audio_binding: std::sync::Mutex<Option<(MediaAttachmentHandle, String, u64)>>,
    media_cloud_audio_refresh: tokio::sync::Mutex<()>,
    /// Settings-visible provider model directory assembled from the full
    /// provider config, including providers not currently routable.
    provider_model_catalog_listings: Vec<lingxi_core::host::ModelListing>,
    official_catalog: Option<harness_runtime::StartupCatalog>,
    catalog_refresh: Option<provider_config::startup_catalog::CatalogRefreshGuard>,
    catalog_observers:
        std::sync::Mutex<HashMap<usize, provider_config::startup_catalog::CatalogRefreshGuard>>,
    /// Packaged Desktop owns persistence in its signed credential broker; in
    /// that mode bridge credential mutations update only this process cache.
    provider_credentials_ephemeral: bool,
    /// The Codex session packaged Desktop keeps fresh. Credential commands
    /// for the Codex provider serve and clear its access token instead of
    /// touching storage.
    openai_host_session: Option<Arc<llm_runtime::auth::openai::HostSession>>,
    /// HTTP transport used by provider connection probes. Production boot
    /// supplies the same guarded transport as the runtime's LLM clients.
    http: Option<Arc<dyn lingxi_core::host::HttpTransport>>,
    /// Optional layered-settings context backing the `Settings` listing.
    /// Production boot wires it from the desktop composition root; lightweight
    /// users of the routing seam may omit it, in which case the listing
    /// reports the missing context rather than emitting nothing.
    settings: Option<SettingsContext>,
    /// Optional MCP-scope roots backing `UpsertMcpServer` / `RemoveMcpServer`.
    /// Production boot wires it from the SAME `.mcp.json` / global-config
    /// paths `harness_runtime::desktop` resolves the read-side registry from;
    /// lightweight users of the routing seam may omit it, in which case the
    /// two commands report the missing context rather than doing nothing.
    mcp: Option<McpPaths>,
    /// Live MCP registry for Desktop-only configuration snapshots and hot reloads.
    mcp_registry: Option<Arc<mcp::McpRegistry>>,
    /// Live plugin runtime for Desktop-only refresh after install/config changes.
    plugin_runtime: Option<Arc<harness_runtime::desktop::PluginRuntime>>,
    /// Live hook registry for source-scoped hot swaps.
    hook_registry: Option<Arc<RwLock<hooks::HookRegistry>>>,
    /// Existing repo-root catalog reloader used for skills/plugins live refresh.
    repo_root_reloader: Option<Arc<dyn lingxi_core::host::RepoRootReloader>>,
    /// FileChanged watcher controller used to replace watch matchers after hook edits.
    file_changed_watcher:
        Option<harness_runtime::desktop::file_changed_watch::FileChangedWatcherController>,
    /// Set while a turn is in flight — `ClearSession` is rejected in this window
    /// (plan §2 mid-turn semantics).
    turn_active: AtomicBool,
}

impl EngineCommandRouter {
    pub(crate) fn with_session_cron(mut self, scheduler: Option<Arc<cron::CronScheduler>>) -> Self {
        self.session_cron = scheduler;
        self
    }
    pub(crate) fn with_cron_firer(mut self, firer: Arc<crate::cron_host::HostCronFirer>) -> Self {
        self.cron_firer = Some(firer);
        self
    }

    /// Install the ready parent and isolated factory used by side-chat wire commands.
    pub(crate) fn with_side_chat_runtime(
        mut self,
        parent: Arc<orchestrator::ConversationOrchestrator>,
        factory: Arc<dyn harness_runtime::side_chat::SideChatFactory>,
    ) -> Self {
        self.side_chat_runtime = Some((parent, factory));
        self
    }

    async fn side_chat_manager(
        &self,
        owner: Arc<dyn ClientEventSink>,
    ) -> Result<Arc<harness_runtime::side_chat::SideChatManager>, String> {
        let mut manager = self.side_chat_manager.lock().await;
        if let Some(manager) = manager.as_ref() {
            return Ok(manager.clone());
        }
        let Some((parent, factory)) = &self.side_chat_runtime else {
            return Err("side chat is unavailable on this bridge".into());
        };
        let created = harness_runtime::side_chat::SideChatManager::new(
            parent.clone(),
            factory.clone(),
            owner,
        );
        *manager = Some(created.clone());
        Ok(created)
    }

    /// Move this process's live-session presence only after the engine has
    /// successfully activated the destination session.  The bridge process
    /// owns one mutable presence record, while durable coordinators retain
    /// old-session leases independently for late background work; updating
    /// this record must therefore never unregister/release the old writer.
    async fn refresh_process_session_presence(
        &self,
        previous: lingxi_core::types::SessionId,
        current: lingxi_core::types::SessionId,
    ) -> Option<String> {
        if let Some(observer) = &self.session_agent_observer {
            observer.set_session_id(current.as_uuid().to_string());
        }
        // Collect, never early-return: the presence record is independent of the
        // cron scheduler, and skipping it leaves this PID advertising the OLD
        // session after a switch that already committed.
        let mut problems: Vec<String> = Vec::new();
        if let Some(scheduler) = &self.session_cron {
            if let Err(error) = scheduler
                .set_session_id(current.as_uuid().to_string())
                .await
            {
                problems.push(format!("scheduled tasks could not restart: {error}"));
            }
        }
        if let Err(error) =
            harness_runtime::desktop::refresh_process_session_presence(previous, current).await
        {
            problems.push(error.to_string());
        }
        (!problems.is_empty()).then(|| format!("session switched, but {}", problems.join("; ")))
    }

    async fn dispatch_desktop_slash(
        &self,
        raw: &Utf16JsonProjection,
    ) -> Option<lingxi_core::host::SlashDispatchResult> {
        let parsed = parse_slash_command(raw)?;
        if source_text_eq_ascii(&parsed.name, "diff") {
            let focus = parsed.raw_args;
            let prompt: Utf16JsonProjection = if source_text_is_empty(&focus) {
                "Inspect the current working tree and show the user the uncommitted diff. Do not modify any files.".into()
            } else {
                let prefix: Utf16JsonProjection = "Inspect the current working tree and show the user the uncommitted diff, focusing on: ".into();
                let suffix: Utf16JsonProjection = ". Do not modify any files.".into();
                concat_source_text(&[&prefix, &focus, &suffix])
            };
            return Some(lingxi_core::host::SlashDispatchResult::RunAsTurn { prompt });
        }
        if source_text_eq_ascii(&parsed.name, "plan") {
            return Some(self.dispatch_desktop_plan(&parsed.raw_args).await);
        }
        None
    }

    async fn resume_replayed_session(
        &self,
        uuid: uuid::Uuid,
        mut replayed: orchestrator::ReplayedSession,
        sink: Arc<dyn ClientEventSink>,
    ) -> Result<(), String> {
        if let Some(workspace) = &self.workspace_commands {
            workspace.discard_cold_resume();
        }
        let resume_plan_mode = replayed.state.plan_mode;
        let previous_session_id = self.handle.current_session_id().await;
        let previous_permission_mode = self
            .handle
            .permission_mode()
            .await
            .unwrap_or_else(|| "default".to_string());
        let previous_plan_mode = self.handle.plan_mode().await;
        let runtime_snapshot = replayed.handle_runtime_snapshot();
        let restored_usage = runtime_snapshot.current_usage;
        if resume_plan_mode {
            self.handle
                .set_permission_mode("plan")
                .await
                .map_err(|error| format!("resume plan permission mode failed: {error}"))?;
            if let Err(error) = self.handle.set_plan_mode(true).await {
                let _ = self
                    .handle
                    .set_permission_mode(&previous_permission_mode)
                    .await;
                return Err(format!("resume plan mode failed: {error}"));
            }
        }
        if let Err(error) = self
            .handle
            .resume_session(
                lingxi_core::types::SessionId::from_uuid(uuid),
                std::mem::take(&mut replayed.state.history),
                replayed.last_message_uuid.map(|value| value.to_string()),
                replayed.state.active_goal.take().map(|goal| {
                    lingxi_core::host::ActiveGoalSnapshot {
                        condition: goal.condition,
                        set_at: goal.set_at,
                        last_reason: goal.last_reason,
                        iterations: goal.iterations,
                        tokens_at_start: goal.tokens_at_start,
                    }
                }),
                runtime_snapshot,
            )
            .await
        {
            if resume_plan_mode {
                let _ = self.handle.set_plan_mode(previous_plan_mode).await;
                let _ = self
                    .handle
                    .set_permission_mode(&previous_permission_mode)
                    .await;
            }
            return Err(format!("resume_session failed: {error}"));
        }
        if let Some(workspace) = &self.workspace_commands {
            workspace.session_facts().reset(true);
            // Owned resume may append deferred tool results. Publish them with
            // the complete captured display history rather than replacing the
            // streamed results with a pre-activation transcript snapshot.
            workspace.merge_live_resume_display(&mut replayed).await?;
        }
        let messages = client::adapter::lowering::lower_replayed_transcript(&replayed);

        // The device-level reasoning default is the counterpart to
        // Electron's persisted model/permission preferences. Apply it
        // after transcript hydration so an old assistant row cannot
        // silently replace the user's last selected effort on restart.
        let _ = self.restore_persisted_reasoning_selection().await;

        let presence_warning = self
            .refresh_process_session_presence(
                previous_session_id,
                lingxi_core::types::SessionId::from_uuid(uuid),
            )
            .await;

        self.emit_resumed_session(uuid, messages, restored_usage, presence_warning, sink)
            .await;
        Ok(())
    }

    async fn emit_resumed_session(
        &self,
        uuid: uuid::Uuid,
        messages: Vec<client::protocol::message::MessageDto>,
        restored_usage: Option<lingxi_core::host::CurrentUsageSnapshot>,
        presence_warning: Option<String>,
        sink: Arc<dyn ClientEventSink>,
    ) {
        let _ = self.refresh_media_cloud_audio_binding().await;
        sink.emit(ClientEvent::SessionResumed {
            session_id: uuid.to_string(),
            mode: SessionModeDto::Code,
            messages,
        })
        .await;
        if let Some(usage) = restored_usage {
            sink.emit(client::adapter::lowering::lower_current_usage(usage))
                .await;
        }
        if let Some(message) = presence_warning {
            sink.emit(ClientEvent::SystemNotice {
                message: message.into(),
                is_error: false,
            })
            .await;
        }
        // resume_session restores the model/effort held by this exact
        // transcript. Publish that authoritative session state after
        // activation so clients do not keep showing the model selected
        // in whichever session happened to be open previously.
        let status = self.handle.get_status_snapshot().await;
        sink.emit(ClientEvent::StatusSnapshot {
            snapshot: lower_status_snapshot(&status),
        })
        .await;
        if !status.model.is_empty() {
            sink.emit(ClientEvent::ModelChanged {
                model: lingxi_core::host::qualified_model_ref(
                    &status.model,
                    status.model_profile.as_deref(),
                ),
                model_source: status.model_source.as_ref().and_then(|source| {
                    client::protocol::text::ClientTextDto::from_projection(source).ok()
                }),
            })
            .await;
        }
        self.emit_controls_snapshot(&*sink).await;
    }

    async fn dispatch_desktop_plan(
        &self,
        args: &Utf16JsonProjection,
    ) -> lingxi_core::host::SlashDispatchResult {
        let previous_permission = self
            .handle
            .permission_mode()
            .await
            .unwrap_or_else(|| "default".to_string());
        let was_plan_mode = self.handle.plan_mode().await;

        if previous_permission != "plan" {
            if let Err(error) = self.handle.set_permission_mode("plan").await {
                return lingxi_core::host::SlashDispatchResult::Handled {
                    display: format!("Could not enable plan mode: {error}").into(),
                };
            }
            lingxi_core::host::live_sessions::set_process_permission_mode("plan", false);
        }
        if !was_plan_mode {
            if let Err(error) = self.handle.set_plan_mode(true).await {
                if previous_permission != "plan" {
                    let _ = self.handle.set_permission_mode(&previous_permission).await;
                    lingxi_core::host::live_sessions::set_process_permission_mode(
                        &previous_permission,
                        previous_permission == "bypassPermissions",
                    );
                }
                return lingxi_core::host::SlashDispatchResult::Handled {
                    display: format!("Could not enable plan mode: {error}").into(),
                };
            }
        }

        if source_text_is_empty(args)
            || source_text_eq_ascii(args, "open")
            || source_text_eq_ascii(args, "share")
        {
            let display = if was_plan_mode {
                "Plan mode is already enabled. The current plan is shown above the composer."
            } else {
                "Plan mode enabled. The next request will be planned before any changes are made."
            };
            lingxi_core::host::SlashDispatchResult::Handled {
                display: display.into(),
            }
        } else {
            lingxi_core::host::SlashDispatchResult::RunAsTurn {
                prompt: args.clone(),
            }
        }
    }

    /// Build a router from the engine handles a desktop runtime exposes.
    #[must_use]
    pub fn new(
        handle: Arc<dyn OrchestratorHandle>,
        auth: Arc<dyn AuthHandle>,
        tasks: Arc<dyn TaskRegistryHandle>,
        dispatcher: Option<Arc<dyn SlashCommandDispatcher>>,
        slash_registry: Option<Arc<RwLock<CommandRegistry>>>,
    ) -> Self {
        Self {
            workspace_commands: None,
            cron_firer: None,
            session_cron: None,
            handle,
            auth,
            tasks,
            dispatcher,
            slash_registry,
            side_chat_runtime: None,
            side_chat_manager: tokio::sync::Mutex::new(None),
            session_store: None,
            session_agent_observer: None,
            team_registry: None,
            catalog_registry: harness_runtime::desktop::FusionCatalogRegistry::default(),
            credentials: None,
            realtime_audio: None,
            media_cloud_audio: None,
            media_cloud_attachment_slot: None,
            media_cloud_audio_provider: None,
            media_cloud_audio_binding: std::sync::Mutex::new(None),
            media_cloud_audio_refresh: tokio::sync::Mutex::new(()),
            provider_model_catalog_listings: Vec::new(),
            official_catalog: None,
            catalog_refresh: None,
            catalog_observers: Default::default(),
            provider_credentials_ephemeral: false,
            openai_host_session: None,
            http: None,
            settings: None,
            mcp: None,
            mcp_registry: None,
            plugin_runtime: None,
            hook_registry: None,
            repo_root_reloader: None,
            file_changed_watcher: None,
            turn_active: AtomicBool::new(false),
        }
    }

    pub(crate) fn with_workspace_commands(
        mut self,
        commands: Arc<crate::workspace_commands::WorkspaceCommands>,
    ) -> Self {
        self.workspace_commands = Some(commands);
        self
    }

    /// Attach the runtime's shared credential manager so Desktop credential
    /// operations use the same secure-store entries as CLI and TUI.
    #[must_use]
    pub fn with_catalog_registry(
        mut self,
        registry: harness_runtime::desktop::FusionCatalogRegistry,
    ) -> Self {
        self.catalog_registry = registry;
        self
    }

    /// Install the real host-owned native Agent realtime controller.
    pub fn with_realtime_audio(
        mut self,
        controller: Arc<crate::realtime_audio::RealtimeAudioController>,
    ) -> Self {
        self.realtime_audio = Some(controller);
        self
    }

    /// Bind the app registry to the host's live attachment slot and the exact
    /// provider host assembled from this Desktop runtime. Owner transitions
    /// replace the slot only after old attachment drain is confirmed.
    pub fn with_media_cloud_audio_registry(
        mut self,
        registry: Arc<AudioProviderCloudRegistry>,
        attachment_slot: Arc<std::sync::RwLock<Option<MediaAttachmentHandle>>>,
        provider: Arc<AudioProviderHost>,
        initial_binding: Option<(MediaAttachmentHandle, String, u64)>,
    ) -> Self {
        self.media_cloud_audio = Some(registry);
        self.media_cloud_attachment_slot = Some(attachment_slot);
        self.media_cloud_audio_provider = Some(provider);
        *self
            .media_cloud_audio_binding
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = initial_binding;
        self
    }

    async fn refresh_media_cloud_audio_binding(&self) -> Result<(String, String), String> {
        let _refresh = self.media_cloud_audio_refresh.lock().await;
        let result = match self.realtime_audio.as_ref() {
            Some(controller) => controller.session_context().await,
            None => Err("realtime audio controller is unavailable".to_owned()),
        };
        self.update_media_cloud_audio_binding_locked(result.clone())
            .await;
        result
    }

    async fn update_media_cloud_audio_binding_locked(
        &self,
        result: Result<(String, String), String>,
    ) {
        let (Some(registry), Some(attachment_slot)) = (
            self.media_cloud_audio.as_ref(),
            self.media_cloud_attachment_slot.as_ref(),
        ) else {
            return;
        };
        let attachment = attachment_slot
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let Some(attachment) = attachment else {
            // No current handle means the host has closed admission. Retain the
            // last route record until its confirmed owner teardown or host drop.
            return;
        };
        match attachment.phase() {
            MediaAttachmentPhase::Revoking | MediaAttachmentPhase::Unknown => {
                // Keep the current route available to already-admitted work
                // while the attachment drain remains unconfirmed.
                return;
            }
            MediaAttachmentPhase::Revoked => {
                registry.unregister_attachment(attachment.id());
                let mut current = self
                    .media_cloud_audio_binding
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if current
                    .as_ref()
                    .is_some_and(|(bound, _, _)| bound.id() == attachment.id())
                {
                    current.take();
                }
                return;
            }
            MediaAttachmentPhase::Active => {}
        }
        let previous_binding = self
            .media_cloud_audio_binding
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        if previous_binding.as_ref().is_some_and(|(previous, _, _)| {
            previous.id() != attachment.id() && previous.phase() != MediaAttachmentPhase::Revoked
        }) {
            // Never publish a replacement route while a different attachment's
            // drain is unconfirmed, even if the slot was changed unexpectedly.
            return;
        }
        let current_session_id = self.handle.current_session_id().await.as_uuid().to_string();
        match result {
            Ok((session_id, profile_name))
                if session_id == current_session_id && !profile_name.trim().is_empty() =>
            {
                let owner = MediaOwner::Session {
                    session_id: session_id.clone(),
                };
                let provider = self.media_cloud_audio_provider.as_ref().cloned();
                match (attachment.authorize_owner(&owner), provider) {
                    (Ok(permit), Some(provider)) => {
                        let profile_result = registry
                            .register_attached_shared_profiles(&permit)
                            .and_then(|()| {
                                registry.register_attached_profile(
                                    &permit,
                                    profile_name.clone(),
                                    provider.clone(),
                                )
                            });
                        let route_result = profile_result.and_then(|()| {
                            registry.register_attached_session(
                                &permit,
                                profile_name.clone(),
                                format!("profile:{profile_name}"),
                                provider,
                            )
                        });
                        match route_result {
                            Ok(generation) => {
                                let previous = self
                                    .media_cloud_audio_binding
                                    .lock()
                                    .unwrap_or_else(|error| error.into_inner())
                                    .replace((attachment.clone(), session_id, generation));
                                if let Some((old_attachment, old_session, old_generation)) =
                                    previous
                                {
                                    if old_attachment.id() == attachment.id() {
                                        registry.unregister_attached_session_if_generation(
                                            old_attachment.id(),
                                            &old_session,
                                            old_generation,
                                        );
                                    } else if old_attachment.phase()
                                        == MediaAttachmentPhase::Revoked
                                    {
                                        registry.unregister_attachment(old_attachment.id());
                                    }
                                }
                                return;
                            }
                            Err(error) => {
                                tracing::debug!(%error, "current attachment has no configured media audio route")
                            }
                        }
                    }
                    (Err(error), _) => {
                        tracing::debug!(%error, "current media attachment no longer admits provider routing")
                    }
                    (_, None) => tracing::debug!("desktop media provider host is unavailable"),
                }
            }
            Ok(_) => {
                tracing::warn!("realtime audio context does not match the current desktop session")
            }
            Err(message) => {
                tracing::debug!(%message, "current session has no authoritative media audio profile")
            }
        }

        let previous = self
            .media_cloud_audio_binding
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some((old_attachment, session_id, generation)) = previous {
            if old_attachment.id() == attachment.id() {
                registry.unregister_attached_session_if_generation(
                    old_attachment.id(),
                    &session_id,
                    generation,
                );
            } else if old_attachment.phase() == MediaAttachmentPhase::Revoked {
                registry.unregister_attachment(old_attachment.id());
            }
        }
    }

    pub fn with_credentials(mut self, credentials: Arc<secret::CredentialManager>) -> Self {
        self.credentials = Some(credentials);
        self
    }

    /// Route the Codex provider's credential commands to the session the
    /// desktop keeps fresh.
    #[must_use]
    pub fn with_openai_host_session(
        mut self,
        session: Option<Arc<llm_runtime::auth::openai::HostSession>>,
    ) -> Self {
        self.openai_host_session = session;
        self
    }

    #[must_use]
    pub fn with_provider_model_catalog_listings(
        mut self,
        listings: Vec<lingxi_core::host::ModelListing>,
    ) -> Self {
        self.provider_model_catalog_listings = listings;
        self
    }

    pub fn with_official_catalog(
        mut self,
        source: Option<harness_runtime::StartupCatalog>,
    ) -> Self {
        self.catalog_refresh = source.as_ref().map(|source| source.prefetch());
        self.official_catalog = source;
        self
    }

    fn observe_catalog(&self, sink: Arc<dyn ClientEventSink>) {
        let Some(source) = &self.official_catalog else {
            return;
        };
        let key = Arc::as_ptr(&sink) as *const () as usize;
        let mut observers = self
            .catalog_observers
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        observers.retain(|_, guard| !guard.0.is_finished());
        if observers.contains_key(&key) {
            return;
        }
        let sink = Arc::downgrade(&sink);
        let source = source.clone();
        let mut updates = source.subscribe();
        let handle = self.handle.clone();
        observers.insert(
            key,
            provider_config::startup_catalog::CatalogRefreshGuard(tokio::spawn(async move {
                while updates.changed().await.is_ok() {
                    let Some(sink) = sink.upgrade() else {
                        break;
                    };
                    let catalog = source.snapshot();
                    sink.emit(ClientEvent::ProviderModelCatalog {
                        providers: harness_runtime::provider_directory::entries(&catalog),
                    })
                    .await;
                    let listings = handle.list_model_listings().await;
                    let snapshot = handle.get_status_snapshot().await;
                    let available = handle.list_available_models().await;
                    sink.emit(ClientEvent::ModelList {
                        models: lingxi_core::host::curated_model_refs(
                            &listings,
                            &available,
                            &snapshot.model,
                            snapshot.model_profile.as_deref(),
                        ),
                        current: lingxi_core::host::qualified_model_ref(
                            &snapshot.model,
                            snapshot.model_profile.as_deref(),
                        ),
                        current_model_source: snapshot
                            .model_source
                            .as_ref()
                            .and_then(|source| ClientTextDto::from_projection(source).ok()),
                        details: lingxi_core::host::curated_model_listings(
                            &listings,
                            &snapshot.model,
                            snapshot.model_profile.as_deref(),
                        )
                        .iter()
                        .map(client::adapter::lowering::lower_model_details)
                        .collect(),
                    })
                    .await;
                }
            })),
        );
    }

    /// Keep provider credential writes process-local because the parent owns
    /// the persistent Data Protection Keychain entry.
    #[must_use]
    pub fn with_ephemeral_provider_credentials(mut self, enabled: bool) -> Self {
        self.provider_credentials_ephemeral = enabled;
        self
    }

    /// Attach the runtime's provider HTTP transport for low-cost connection
    /// probes from Desktop settings.
    #[must_use]
    pub fn with_http(mut self, http: Arc<dyn lingxi_core::host::HttpTransport>) -> Self {
        self.http = Some(http);
        self
    }

    /// Share the same process-local liveness source used for pushed agent events.
    #[must_use]
    pub fn with_session_agent_observer(
        mut self,
        observer: Arc<harness_runtime::desktop::session_agents::DesktopSessionAgentObserver>,
    ) -> Self {
        self.session_agent_observer = Some(observer);
        self
    }

    #[must_use]
    pub fn with_team_registry(
        mut self,
        registry: Arc<dyn lingxi_core::host::team_registry::TeamRegistryHandle>,
    ) -> Self {
        self.team_registry = Some(registry);
        self
    }

    /// Attach the persisted JSONL session store used by list/resume commands.
    #[must_use]
    pub fn with_session_store(mut self, session_store: SessionStoreContext) -> Self {
        self.session_store = Some(session_store);
        self
    }

    /// Attach the layered-settings context the `Settings` listing reads. The
    /// composition root supplies the settings roots, the file-layer values read
    /// at session start, and the administrator's managed overlay; this router
    /// only reads and lowers them.
    #[must_use]
    pub fn with_settings_context(mut self, settings: SettingsContext) -> Self {
        self.settings = Some(settings);
        self
    }

    /// Attach the MCP-scope roots the `UpsertMcpServer` / `RemoveMcpServer`
    /// commands write through. The composition root supplies the SAME
    /// project directory and global-config path the read-side registry was
    /// loaded from (`resolve_desktop_config`'s `project_mcp_path` /
    /// `global_mcp_path`), so a write always lands where the next reload
    /// would look for it.
    #[must_use]
    pub fn with_mcp_paths(mut self, mcp: McpPaths) -> Self {
        self.mcp = Some(mcp);
        self
    }

    /// Attach the live MCP registry for Desktop configuration snapshots and
    /// hot reconnect/replacement.
    #[must_use]
    pub fn with_mcp_registry(mut self, mcp_registry: Arc<mcp::McpRegistry>) -> Self {
        self.mcp_registry = Some(mcp_registry);
        self
    }

    /// Attach the live plugin runtime so Desktop settings can refresh plugin
    /// registries after a successful mutation.
    #[must_use]
    pub fn with_plugin_runtime(
        mut self,
        plugin_runtime: Option<Arc<harness_runtime::desktop::PluginRuntime>>,
    ) -> Self {
        self.plugin_runtime = plugin_runtime;
        self
    }

    /// Attach the live hook registry for Desktop hook hot swaps.
    #[must_use]
    pub fn with_hook_registry(mut self, hook_registry: Arc<RwLock<hooks::HookRegistry>>) -> Self {
        self.hook_registry = Some(hook_registry);
        self
    }

    /// Attach the repo-root reloader for skills/plugins live refreshes.
    #[must_use]
    pub fn with_repo_root_reloader(
        mut self,
        repo_root_reloader: Arc<dyn lingxi_core::host::RepoRootReloader>,
    ) -> Self {
        self.repo_root_reloader = Some(repo_root_reloader);
        self
    }

    /// Attach the FileChanged watcher controller for replacing settings-derived
    /// matcher sets after hook edits.
    #[must_use]
    pub fn with_file_changed_watcher(
        mut self,
        watcher: Option<harness_runtime::desktop::file_changed_watch::FileChangedWatcherController>,
    ) -> Self {
        self.file_changed_watcher = watcher;
        self
    }

    /// Mark whether a turn is currently in flight. The connection flips this
    /// `true` when it spawns a turn on `SendPrompt` and `false` when the turn
    /// ends; `ClearSession` is rejected while it is `true`.
    pub fn set_turn_active(&self, active: bool) {
        self.turn_active.store(active, Ordering::SeqCst);
    }

    /// True iff a turn is currently in flight.
    #[must_use]
    pub fn is_turn_active(&self) -> bool {
        self.turn_active.load(Ordering::SeqCst)
    }

    async fn dispatch_mod_ui_control_result(
        &self,
        request_id: String,
        request: Result<lingxi_core::types::utf16_json::Utf16JsonProjection, String>,
        sink: &dyn ClientEventSink,
    ) {
        let result = match request {
            Ok(request) => self.handle.mod_ui_control(request).await,
            Err(error) => Err(HandleError::ActionFailed(error)),
        };
        sink.emit(client::adapter::turn::mod_ui_control_result_event(
            request_id, result,
        ))
        .await;
    }

    async fn dispatch_mod_ui_client_operation_result(
        &self,
        request_id: String,
        operation: Result<lingxi_core::types::utf16_json::Utf16JsonProjection, String>,
        sink: &dyn ClientEventSink,
    ) {
        let result = match operation {
            Ok(operation) => self.handle.mod_ui_client_operation(operation).await,
            Err(error) => Err(HandleError::ActionFailed(error)),
        };
        sink.emit(client::adapter::turn::mod_ui_client_operation_result_event(
            request_id, result,
        ))
        .await;
    }
}

#[async_trait]
impl CommandRouter for EngineCommandRouter {
    async fn resume_selected_session(
        &self,
        request: Utf16JsonProjection,
        sink: Arc<dyn ClientEventSink>,
    ) -> Result<Utf16JsonProjection, String> {
        if self.is_turn_active() {
            return Err("cannot resume while a turn is in flight".into());
        }
        let id = request
            .string_at("/session_id")
            .and_then(|value| uuid::Uuid::parse_str(value).ok())
            .ok_or("resume_selected_session requires a valid session_id")?;
        if self.handle.current_session_id().await.as_uuid() != id {
            return Err("resume_selected_session must retain the active session UUID".into());
        }
        let path = request
            .string_at("/transcript_path")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .ok_or("resume_selected_session requires an absolute transcript_path")?;
        if let Some(workspace) = &self.workspace_commands {
            if let Some(proof) = workspace.complete_cold_resume(id, &path).await? {
                self.emit_resumed_session(id, proof.messages, proof.current_usage, None, sink)
                    .await;
                return Utf16JsonProjection::parse(r#"{"status":"ok"}"#)
                    .map_err(|error| error.to_string());
            }
        }
        let store = self
            .session_store
            .as_ref()
            .ok_or("resume: persisted session store is unavailable")?;
        let replayed =
            orchestrator::resume::replay_session_state_from_path(&path, id, store.fs.clone())
                .await
                .map_err(|error| format!("resume: selected session is not resumable: {error}"))?;
        self.resume_replayed_session(id, replayed, sink).await?;
        Utf16JsonProjection::parse(r#"{"status":"ok"}"#).map_err(|error| error.to_string())
    }

    async fn add_working_directory(
        &self,
        request: Utf16JsonProjection,
        sink: Arc<dyn ClientEventSink>,
    ) -> Result<Utf16JsonProjection, String> {
        let before = self.capture_slash_authority().await;
        let commands = self
            .workspace_commands
            .as_ref()
            .ok_or("add_directory is unavailable")?;
        let result = commands.add_working_directory(request).await;
        let (display, is_error) = match &result {
            Ok((_, display)) => (display.clone(), false),
            Err(display) => (display.clone(), true),
        };
        sink.emit(ClientEvent::SlashCommandResult {
            turn_id: None,
            display: ClientTextDto::from_projection(&display)
                .expect("directory command display is text"),
            is_error,
        })
        .await;
        let after = self.capture_slash_authority().await;
        self.emit_slash_authority_changes(&before, &after, sink.as_ref())
            .await;
        result.map(|(state, _)| state).map_err(|display| {
            display
                .string_at("")
                .unwrap_or("The directory command failed.")
                .to_owned()
        })
    }
    async fn workspace_state(&self) -> Result<Utf16JsonProjection, String> {
        self.workspace_commands
            .as_ref()
            .ok_or("workspace state is unavailable")?
            .state()
            .await
    }
    async fn set_working_directory(
        &self,
        request: Utf16JsonProjection,
        sink: Arc<dyn ClientEventSink>,
    ) -> Result<Utf16JsonProjection, String> {
        let commands = self
            .workspace_commands
            .as_ref()
            .ok_or("set_cwd is unavailable")?;
        let result = commands.set_working_directory(request).await?;
        if result.string_at("/status") == Some("ok") {
            sink.emit(ClientEvent::StatusSnapshot {
                snapshot: lower_status_snapshot(&self.handle.get_status_snapshot().await),
            })
            .await;
        }
        Ok(result)
    }
    async fn restore_workspace_controls(
        &self,
        request: Utf16JsonProjection,
        sink: Arc<dyn ClientEventSink>,
    ) -> Result<Utf16JsonProjection, String> {
        let result = self
            .workspace_commands
            .as_ref()
            .ok_or("workspace controls are unavailable")?
            .restore_runtime_controls(request)
            .await?;
        self.emit_controls_snapshot(sink.as_ref()).await;
        Ok(result)
    }
    fn realtime_audio_supported(&self) -> bool {
        self.realtime_audio.is_some()
    }
    fn abort_realtime_audio(&self) {
        if let Some(controller) = &self.realtime_audio {
            controller.abort_now();
        }
    }
    async fn stop_realtime_audio(&self) {
        if let Some(controller) = &self.realtime_audio {
            controller.stop().await;
        }
    }
    async fn audio_session_context(&self) -> (Option<String>, Option<String>, Option<String>) {
        let current = self.handle.current_session_id().await.as_uuid().to_string();
        let profile = self
            .refresh_media_cloud_audio_binding()
            .await
            .ok()
            .and_then(|(session, profile)| (session == current).then_some(profile));
        let scope = profile.as_ref().map(|profile| format!("profile:{profile}"));
        (Some(current), profile, scope)
    }
    async fn prepare_media_attachment(
        &self,
        attachment: &MediaAttachmentHandle,
        owner: &MediaOwner,
    ) -> Result<(), String> {
        let _refresh = self.media_cloud_audio_refresh.lock().await;
        let (Some(registry), Some(controller), Some(provider)) = (
            self.media_cloud_audio.as_ref(),
            self.realtime_audio.as_ref(),
            self.media_cloud_audio_provider.as_ref(),
        ) else {
            return Ok(());
        };
        let (session_id, profile_name) = match controller.session_context().await {
            Ok(context) => context,
            Err(_) => {
                let mut current = self
                    .media_cloud_audio_binding
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if current
                    .as_ref()
                    .is_some_and(|(old, _, _)| old.phase() == MediaAttachmentPhase::Revoked)
                {
                    current.take();
                }
                return Ok(()); // no configured cloud audio for this session
            }
        };
        let expected = MediaOwner::Session {
            session_id: session_id.clone(),
        };
        if owner != &expected
            || self.handle.current_session_id().await.as_uuid().to_string() != session_id
            || profile_name.trim().is_empty()
        {
            return Err("media provider context does not match the replacement session".into());
        }
        if self
            .media_cloud_audio_binding
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|(previous, _, _)| {
                previous.id() != attachment.id()
                    && previous.phase() != MediaAttachmentPhase::Revoked
            })
        {
            return Err("previous media provider attachment remains unconfirmed".into());
        }
        let permit = attachment
            .authorize_owner(owner)
            .map_err(|e| e.to_string())?;
        registry
            .register_attached_shared_profiles(&permit)
            .map_err(|e| e.to_string())?;
        registry
            .register_attached_profile(&permit, profile_name.clone(), provider.clone())
            .map_err(|e| e.to_string())?;
        let generation = registry
            .register_attached_session(
                &permit,
                profile_name.clone(),
                format!("profile:{profile_name}"),
                provider.clone(),
            )
            .map_err(|e| e.to_string())?;
        let previous = self
            .media_cloud_audio_binding
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .replace((attachment.clone(), session_id, generation));
        if let Some((old_attachment, old_session, old_generation)) = previous {
            if old_attachment.id() == attachment.id() {
                registry.unregister_attached_session_if_generation(
                    old_attachment.id(),
                    &old_session,
                    old_generation,
                );
            } else if old_attachment.phase() == MediaAttachmentPhase::Revoked {
                registry.unregister_attachment(old_attachment.id());
            }
        }
        Ok(())
    }
    async fn cleanup_confirmed_media_binding(&self) {
        let _refresh = self.media_cloud_audio_refresh.lock().await;
        let mut current = self
            .media_cloud_audio_binding
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if current
            .as_ref()
            .is_some_and(|(old, _, _)| old.phase() == MediaAttachmentPhase::Revoked)
        {
            if let Some((old, _, _)) = current.take() {
                if let Some(registry) = self.media_cloud_audio.as_ref() {
                    registry.unregister_attachment(old.id());
                }
            }
        }
    }
    async fn connection_closed(&self) {
        let manager = self.side_chat_manager.lock().await.take();
        if let Some(manager) = manager {
            if let Err(error) = manager.shutdown().await {
                tracing::warn!(%error, "side-chat shutdown reported cleanup failures");
            }
        }
    }
    async fn runtime_snapshot(&self) -> Result<Vec<ClientEvent>, String> {
        if self.team_registry.is_none() {
            return Err("desktop runtime snapshot is unavailable".into());
        }
        let events = RuntimeSnapshotEvents::default();
        self.emit_session_agent_list(&events).await;
        self.emit_coordinator_snapshot(&events).await;
        tasks::emit_correlated_task_list(
            self.tasks.as_ref(),
            &events,
            TaskListFilter::default(),
            "desktop-runtime-snapshot".into(),
        )
        .await;
        let events = events
            .0
            .into_inner()
            .unwrap_or_else(|poison| poison.into_inner());
        if !events
            .iter()
            .any(|event| matches!(event, ClientEvent::SessionAgentList { .. }))
        {
            return Err("session agent snapshot is unavailable".into());
        }
        if let Some(error) = events.iter().find_map(|event| match event {
            ClientEvent::TaskListComplete {
                error: Some(error), ..
            } => Some(error),
            _ => None,
        }) {
            return Err(format!("desktop task snapshot is unavailable: {error}"));
        }
        Ok(events)
    }

    async fn dispatch_slash(
        &self,
        raw: Utf16JsonProjection,
        cancellation: CancellationToken,
    ) -> Option<SlashDispatchOutcome> {
        let before = self.capture_slash_authority().await;
        let is_cd = parse_slash_command(&raw)
            .is_some_and(|parsed| source_text_eq_ascii(&parsed.name, "cd"));
        let result = match self.dispatch_desktop_slash(&raw).await {
            Some(result) => result,
            None => self.dispatcher.as_ref()?.dispatch(raw, cancellation).await,
        };
        // Display-only feedback belongs to the client's command surface. Persist
        // actual task turns and command effects through their existing paths.
        let after = self.capture_slash_authority().await;
        if before.session_id != after.session_id {
            if let Some(workspace) = &self.workspace_commands {
                workspace.session_facts().reset(false);
            }
        }
        let mut authority_events = Self::slash_authority_change_events(&before, &after);
        if is_cd
            && matches!(
                &result,
                lingxi_core::host::SlashDispatchResult::Handled { .. }
            )
        {
            authority_events.push(ClientEvent::StatusSnapshot {
                snapshot: lower_status_snapshot(&self.handle.get_status_snapshot().await),
            });
        }
        Some(SlashDispatchOutcome {
            result,
            authority_events,
        })
    }

    async fn requires_idle_turn_for_slash(&self, raw: &Utf16JsonProjection) -> bool {
        let Some(registry) = self.slash_registry.as_ref() else {
            return false;
        };
        let registry = registry.read().await;
        requires_idle_turn_for_slash(&registry, raw)
    }

    // The full command dispatch is one match over the command surface; splitting
    // it would scatter the one-place-routes-everything map this module exists to
    // be. (Same convention as `harness_runtime::desktop::build`.)
    #[allow(clippy::too_many_lines)]
    async fn route(&self, command: ClientCommand, sink: Arc<dyn ClientEventSink>) {
        self.observe_catalog(sink.clone());
        match command {
            ClientCommand::StartRealtimeAudio { request_json } => {
                if let Some(controller) = &self.realtime_audio {
                    controller.start(request_json, sink).await;
                } else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Rejected,
                        message: "native realtime audio is unavailable on this connection".into(),
                    })
                    .await;
                }
            }
            ClientCommand::RealtimeAudioInput { input_json } => {
                if let Some(controller) = &self.realtime_audio {
                    controller.input(input_json, sink).await;
                } else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Rejected,
                        message: "native realtime audio is unavailable on this connection".into(),
                    })
                    .await;
                }
            }
            ClientCommand::StopRealtimeAudio => {
                self.stop_realtime_audio().await;
            }
            ClientCommand::UiRender {
                request_id,
                request_json,
            } => {
                let request =
                    client::adapter::turn::parse_mod_ui_control_request(&request_json, "ui_render");
                self.dispatch_mod_ui_control_result(request_id, request, sink.as_ref())
                    .await;
            }
            ClientCommand::UiClientModule { request_id, plugin } => {
                let request = Ok(Utf16JsonProjection::object(vec![
                    ("subtype".to_owned(), "ui_client_module".into()),
                    ("plugin".to_owned(), plugin.into()),
                ])
                .expect("generated Mod UI module request is an object"));
                self.dispatch_mod_ui_control_result(request_id, request, sink.as_ref())
                    .await;
            }
            ClientCommand::UiMessage {
                request_id,
                request_json,
            } => {
                let request = client::adapter::turn::parse_mod_ui_control_request(
                    &request_json,
                    "ui_message",
                );
                self.dispatch_mod_ui_control_result(request_id, request, sink.as_ref())
                    .await;
            }
            ClientCommand::UiClientFault {
                request_id,
                request_json,
            } => {
                let request = client::adapter::turn::parse_mod_ui_control_request(
                    &request_json,
                    "ui_client_fault",
                );
                self.dispatch_mod_ui_control_result(request_id, request, sink.as_ref())
                    .await;
            }
            ClientCommand::UiClientPress {
                request_id,
                request_json,
            } => {
                let request = client::adapter::turn::parse_mod_ui_control_request(
                    &request_json,
                    "ui_client_press",
                );
                self.dispatch_mod_ui_control_result(request_id, request, sink.as_ref())
                    .await;
            }
            ClientCommand::UiPress {
                request_id,
                request_json,
            } => {
                let request =
                    client::adapter::turn::parse_mod_ui_control_request(&request_json, "ui_press");
                self.dispatch_mod_ui_control_result(request_id, request, sink.as_ref())
                    .await;
            }
            ClientCommand::UiInput {
                request_id,
                request_json,
            } => {
                let request =
                    client::adapter::turn::parse_mod_ui_control_request(&request_json, "ui_input");
                self.dispatch_mod_ui_control_result(request_id, request, sink.as_ref())
                    .await;
            }
            ClientCommand::UiSelect {
                request_id,
                request_json,
            } => {
                let request =
                    client::adapter::turn::parse_mod_ui_control_request(&request_json, "ui_select");
                self.dispatch_mod_ui_control_result(request_id, request, sink.as_ref())
                    .await;
            }
            ClientCommand::UiClientOperation {
                request_id,
                operation_json,
            } => {
                let operation =
                    client::adapter::turn::parse_mod_ui_client_operation(&operation_json);
                self.dispatch_mod_ui_client_operation_result(request_id, operation, sink.as_ref())
                    .await;
            }
            ClientCommand::SideChatCreate {
                request_id,
                initial_input,
            } => {
                let manager = self.side_chat_manager(sink.clone()).await;
                let result = match manager {
                    Ok(manager) => {
                        manager
                            .create(request_id.clone(), initial_input, None, None)
                            .await
                    }
                    Err(error) => Err(error),
                };
                if let Err(message) = result {
                    sink.emit(ClientEvent::SideChatError {
                        request_id: Some(request_id),
                        side_id: None,
                        message,
                    })
                    .await;
                }
            }
            ClientCommand::SideChatList {} => match self.side_chat_manager(sink.clone()).await {
                Ok(manager) => {
                    sink.emit(ClientEvent::SideChatList {
                        side_ids: manager.list().await,
                    })
                    .await;
                }
                Err(message) => {
                    sink.emit(ClientEvent::SideChatError {
                        request_id: None,
                        side_id: None,
                        message,
                    })
                    .await;
                }
            },
            ClientCommand::SideChatGet { side_id } => {
                match self.side_chat_manager(sink.clone()).await {
                    Ok(manager) => {
                        if let Err(message) = manager.emit_snapshot(&side_id).await {
                            sink.emit(ClientEvent::SideChatError {
                                request_id: None,
                                side_id: Some(side_id),
                                message,
                            })
                            .await;
                        }
                    }
                    Err(message) => {
                        sink.emit(ClientEvent::SideChatError {
                            request_id: None,
                            side_id: Some(side_id),
                            message,
                        })
                        .await;
                    }
                }
            }
            ClientCommand::SideChatEnd { side_id } => {
                match self.side_chat_manager(sink.clone()).await {
                    Ok(manager) => match manager.end(&side_id).await {
                        Ok(()) => {
                            sink.emit(ClientEvent::SideChatEnded { side_id }).await;
                        }
                        Err(message) => {
                            sink.emit(ClientEvent::SideChatError {
                                request_id: None,
                                side_id: Some(side_id),
                                message,
                            })
                            .await;
                        }
                    },
                    Err(message) => {
                        sink.emit(ClientEvent::SideChatError {
                            request_id: None,
                            side_id: Some(side_id),
                            message,
                        })
                        .await;
                    }
                }
            }
            ClientCommand::SideChatSubmit {
                side_id,
                request_id,
                command_json,
            } => {
                let manager = match self.side_chat_manager(sink.clone()).await {
                    Ok(manager) => manager,
                    Err(message) => {
                        sink.emit(ClientEvent::SideChatError {
                            request_id: Some(request_id),
                            side_id: Some(side_id),
                            message,
                        })
                        .await;
                        return;
                    }
                };
                let command = match serde_json::from_str::<ClientCommand>(&command_json) {
                    Ok(command) => command,
                    Err(error) => {
                        sink.emit(ClientEvent::SideChatError {
                            request_id: Some(request_id),
                            side_id: Some(side_id),
                            message: format!("invalid nested side-chat command: {error}"),
                        })
                        .await;
                        return;
                    }
                };
                if let Err(message) = manager.submit(&side_id, request_id.clone(), command).await {
                    sink.emit(ClientEvent::SideChatError {
                        request_id: Some(request_id),
                        side_id: Some(side_id),
                        message,
                    })
                    .await;
                }
            }
            // Both arms write the durable automation store — `started` binds a
            // session id into it, `complete` records a run result — so they sit
            // behind the same trust gate as the `CronManage` mutations below.
            // Without it an untrusted workspace can still mutate the file every
            // other write path refuses to touch.
            ClientCommand::CronRunStarted { run_id, session_id } => {
                if !self.handle.workspace_trusted().await {
                    return;
                }
                if let Some(firer) = &self.cron_firer {
                    firer.started(&run_id, &session_id).await;
                }
            }
            ClientCommand::CronRunCompleted {
                run_id,
                session_id,
                summary,
                error,
            } => {
                if !self.handle.workspace_trusted().await {
                    return;
                }
                if let Some(firer) = &self.cron_firer {
                    firer.complete(&run_id, session_id, summary, error).await;
                }
            }
            ClientCommand::CronManage {
                request_id,
                request,
            } => {
                if !matches!(request.action.as_str(), "list" | "history")
                    && !self.handle.workspace_trusted().await
                {
                    sink.emit(ClientEvent::CronResult {
                        request_id,
                        jobs: Vec::new(),
                        error: Some("Trust this workspace before changing scheduled tasks".into()),
                    })
                    .await;
                    return;
                }
                let status = self.handle.get_status_snapshot().await;
                let owner_id = status
                    .session_id
                    .strip_prefix("sess:")
                    .unwrap_or(&status.session_id);
                if request.action == "create" {
                    let anchor = match self.session_store.as_ref() {
                        Some(store) => store.ensure_scheduled_chat(owner_id).await,
                        None => Err("Scheduled task chat storage is unavailable".into()),
                    };
                    if let Err(error) = anchor {
                        sink.emit(ClientEvent::CronResult {
                            request_id,
                            jobs: Vec::new(),
                            error: Some(error),
                        })
                        .await;
                        return;
                    }
                }
                let cwd = status.cwd;
                let fs = crate::HostFileSystem::new(cwd.clone());
                let result = harness_runtime::desktop::cron_management::manage(
                    &fs,
                    &cwd,
                    request,
                    &self.tasks,
                    owner_id,
                )
                .await;
                let (jobs, error) = match result {
                    Ok(jobs) => (jobs, None),
                    Err(error) => (Vec::new(), Some(error)),
                };
                sink.emit(ClientEvent::CronResult {
                    request_id,
                    jobs,
                    error,
                })
                .await;
            }
            // ── Provider credentials ────────────────────────────────────────
            ClientCommand::ListProviderCredentials {
                operation_id,
                provider_ids,
                preview_provider_ids,
            } => {
                let validation_error = if provider_ids.len() > 32
                    || provider_ids.iter().any(|id| !provider_id_is_valid(id))
                    || preview_provider_ids
                        .iter()
                        .any(|id| !provider_ids.contains(id))
                {
                    Some("invalid provider credential query".to_string())
                } else {
                    None
                };
                self.emit_provider_credential_status(
                    operation_id,
                    &provider_ids,
                    &preview_provider_ids,
                    validation_error,
                    &*sink,
                )
                .await;
            }
            // The desktop refreshes its one Codex session itself and serves
            // every engine the current access token, so nothing is stored here.
            ClientCommand::SetProviderCredential {
                operation_id,
                provider_id,
                credential,
            } if provider_id == crate::boot::CODEX_PROVIDER_ID
                && self.openai_host_session.is_some() =>
            {
                let error = match &self.openai_host_session {
                    Some(session) => serve_openai_host_token(session, credential.expose_secret()),
                    None => Some("Codex session is unavailable".to_string()),
                };
                // The session is the ChatGPT delegate for this engine's whole
                // life, so Fusion adopts the token in place.
                if error.is_none()
                    && !crate::boot::refresh_fusion_catalog_bounded(
                        &self.catalog_registry,
                        vec![provider_id.clone()],
                    )
                    .await
                {
                    tracing::warn!("Fusion did not adopt the Codex session without a restart");
                }
                let applied = error.is_none();
                sink.emit(ClientEvent::ProviderCredentialStatus {
                    operation_id,
                    configured_provider_ids: applied
                        .then_some(provider_id.clone())
                        .into_iter()
                        .collect(),
                    unavailable_provider_ids: (!applied)
                        .then_some(provider_id)
                        .into_iter()
                        .collect(),
                    storage_encrypted: self
                        .credentials
                        .as_ref()
                        .is_some_and(|credentials| credentials.provider_key_storage_is_encrypted()),
                    // Every part of a session is secret; it has no preview.
                    credential_previews: HashMap::new(),
                    error,
                })
                .await;
            }
            ClientCommand::SetProviderCredential {
                operation_id,
                provider_id,
                credential,
            } => {
                let credential_preview =
                    secret::masked_credential_preview(credential.expose_secret());
                let mut error = if !provider_id_is_valid(&provider_id)
                    || credential.expose_secret().is_empty()
                    || credential.expose_secret().len() > 16_384
                    || credential.expose_secret().contains('\0')
                {
                    Some("invalid provider credential".to_string())
                } else if let Some(credentials) = self.credentials.as_ref() {
                    if self.provider_credentials_ephemeral {
                        credentials
                            .set_provider_key_ephemeral(&provider_id, credential.expose_secret())
                            .await;
                        None
                    } else {
                        credentials
                            .set_provider_key(&provider_id, credential.expose_secret())
                            .await
                            .err()
                            .map(|failure| {
                                format!("failed to store provider credential: {failure}")
                            })
                    }
                } else {
                    Some("provider credential storage is unavailable".to_string())
                };
                // Publish before emitting readiness. The helper starts its
                // full-source reconciliation detached; `false` means storage
                // succeeded but this process's fixed auth route needs restart.
                if error.is_none()
                    && !crate::boot::refresh_fusion_catalog_bounded(
                        &self.catalog_registry,
                        vec![provider_id.clone()],
                    )
                    .await
                {
                    error = Some(
                        harness_runtime::desktop::fusion_credential_restart_required_message(
                            &provider_id,
                        ),
                    );
                }
                let applied = error.is_none();
                let credential_previews = if applied {
                    HashMap::from([(provider_id.clone(), credential_preview)])
                } else {
                    HashMap::new()
                };
                sink.emit(ClientEvent::ProviderCredentialStatus {
                    operation_id,
                    configured_provider_ids: applied
                        .then_some(provider_id.clone())
                        .into_iter()
                        .collect(),
                    unavailable_provider_ids: (!applied)
                        .then_some(provider_id)
                        .into_iter()
                        .collect(),
                    storage_encrypted: self
                        .credentials
                        .as_ref()
                        .is_some_and(|credentials| credentials.provider_key_storage_is_encrypted()),
                    credential_previews,
                    error,
                })
                .await;
            }
            // The desktop signed out of Codex.
            ClientCommand::DeleteProviderCredential {
                operation_id,
                provider_id,
            } if provider_id == crate::boot::CODEX_PROVIDER_ID
                && self.openai_host_session.is_some() =>
            {
                if let Some(session) = &self.openai_host_session {
                    session.clear();
                }
                harness_runtime::desktop::refresh_fusion_catalog_after_credential_delete(
                    &self.catalog_registry,
                    &provider_id,
                )
                .await;
                sink.emit(ClientEvent::ProviderCredentialStatus {
                    operation_id,
                    configured_provider_ids: Vec::new(),
                    unavailable_provider_ids: Vec::new(),
                    storage_encrypted: self
                        .credentials
                        .as_ref()
                        .is_some_and(|credentials| credentials.provider_key_storage_is_encrypted()),
                    credential_previews: HashMap::new(),
                    error: None,
                })
                .await;
            }
            // Round-12 finding [2]: a credential DELETE must tell Fusion's
            // catalog filter, and it must NOT do so through the sibling `Set`
            // arm's `crate::boot::refresh_fusion_catalog_bounded` one screen
            // up — that routes to
            // `FusionCatalogRefresher::refresh_after_credential_write`, whose
            // `forced` loop publishes `true` for the named profile, i.e. the
            // exact opposite of what a delete means. The refresher's merge
            // rule is additive by design and can never lower a `true`
            // (`refresh_inner`: `if row.available { insert(true) } else {
            // entry(..).or_insert(false) }`), so a re-probe cannot close this
            // either. Hence the dedicated removal fan-out
            // `harness_runtime::desktop::refresh_fusion_catalog_after_credential_delete`
            // below. Without it a provider whose key was deleted mid-session
            // survived `filter_fusion_catalog`, could be auto-selected as a
            // `/fusion` panel, had budget reserved for it, and died on
            // `LlmError::Authentication` at request time instead of being
            // excluded by the §4 preflight. BOTH branches need it — the
            // persistent keychain delete and the packaged/brokered ephemeral
            // one, whose key never touched the keychain at all and so could
            // never be re-probed away even in principle. Unlike the write
            // path this needs no budget wrapper: `mark_credential_removed` is
            // a lock-and-set with no credential-backend I/O.
            ClientCommand::DeleteProviderCredential {
                operation_id,
                provider_id,
            } => {
                let error = if !provider_id_is_valid(&provider_id) {
                    Some("invalid provider id".to_string())
                } else if let Some(credentials) = self.credentials.as_ref() {
                    if self.provider_credentials_ephemeral {
                        credentials
                            .suppress_provider_key_ephemeral(&provider_id)
                            .await;
                        None
                    } else {
                        credentials
                            .delete_provider_key(&provider_id)
                            .await
                            .err()
                            .map(|failure| {
                                format!("failed to delete provider credential: {failure}")
                            })
                    }
                } else {
                    Some("provider credential storage is unavailable".to_string())
                };
                let applied = error.is_none();
                if applied {
                    harness_runtime::desktop::refresh_fusion_catalog_after_credential_delete(
                        &self.catalog_registry,
                        &provider_id,
                    )
                    .await;
                }
                sink.emit(ClientEvent::ProviderCredentialStatus {
                    operation_id,
                    configured_provider_ids: Vec::new(),
                    unavailable_provider_ids: (!applied)
                        .then_some(provider_id)
                        .into_iter()
                        .collect(),
                    storage_encrypted: self
                        .credentials
                        .as_ref()
                        .is_some_and(|credentials| credentials.provider_key_storage_is_encrypted()),
                    credential_previews: HashMap::new(),
                    error,
                })
                .await;
            }
            ClientCommand::TestProviderConnection {
                operation_id,
                provider_id,
                api_base,
                model,
                credential_override,
            } => {
                let event = if provider_id_is_valid(&provider_id) {
                    crate::provider_connection::test(
                        self.credentials.as_ref(),
                        self.http.as_ref(),
                        crate::provider_connection::ProviderConnectionProbe {
                            protocol: self.official_catalog.as_ref().and_then(|source| {
                                source
                                    .snapshot()
                                    .providers
                                    .iter()
                                    .find(|p| p.profile_name == provider_id)
                                    .and_then(|p| serde_json::to_value(p.protocol).ok())
                                    .and_then(|v| v.as_str().map(str::to_owned))
                            }),
                            operation_id,
                            provider_id,
                            api_base,
                            model,
                            credential_override,
                        },
                    )
                    .await
                } else {
                    ClientEvent::ProviderConnectionTested {
                        operation_id,
                        provider_id,
                        connected: false,
                        reachable: false,
                        authenticated: false,
                        model_available: false,
                        http_status: None,
                        latency_ms: 0,
                        message: "Provider 标识无效".to_string(),
                        used_stored_credential: credential_override.is_none(),
                    }
                };
                sink.emit(event).await;
            }

            // ── Permission mode ────────────────────────────────────────────
            ClientCommand::SetPermissionMode { mode } => {
                match self.handle.set_permission_mode(&mode).await {
                    Ok(()) => {
                        let active = self.handle.permission_mode().await.unwrap_or(mode);
                        lingxi_core::host::live_sessions::set_process_permission_mode(
                            &active,
                            active == "bypassPermissions",
                        );
                        sink.emit(ClientEvent::PermissionModeChanged { mode: active })
                            .await;
                    }
                    Err(e) => {
                        sink.emit(ClientEvent::Error {
                            kind: ErrorKindDto::Rejected,
                            message: format!("set_permission_mode failed: {e}").into(),
                        })
                        .await;
                    }
                }
                self.emit_controls_snapshot(&*sink).await;
            }
            ClientCommand::InitializeProactivity {
                proactivity,
                reinitialize,
            } => match self
                .handle
                .apply_native_proactivity_initialize(proactivity, reinitialize.unwrap_or(false))
                .await
            {
                Ok(()) => self.emit_controls_snapshot(&*sink).await,
                Err(error) => {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Rejected,
                        message: format!("initialize_proactivity failed: {error}").into(),
                    })
                    .await;
                }
            },
            ClientCommand::SetProactivityLevel {
                level,
                offer_bypass,
            } => match self
                .handle
                .set_native_proactivity_level(&level, offer_bypass)
                .await
            {
                Ok(response) => {
                    lingxi_core::host::live_sessions::set_process_permission_mode(
                        &response.permission_mode,
                        response.permission_mode == "bypassPermissions",
                    );
                    sink.emit(ClientEvent::PermissionModeChanged {
                        mode: response.permission_mode,
                    })
                    .await;
                    self.emit_controls_snapshot(&*sink).await;
                }
                Err(error) => {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Rejected,
                        message: format!("set_proactivity_level failed: {error}").into(),
                    })
                    .await;
                }
            },

            // ── Model ──────────────────────────────────────────────────────
            ClientCommand::GetAudioSessionContext { request_id } => {
                let (session_id, profile_id, account_scope) = self.audio_session_context().await;
                sink.emit(ClientEvent::AudioSessionContext {
                    request_id,
                    session_id,
                    profile_id,
                    account_scope,
                    media_owner: None,
                })
                .await;
            }
            ClientCommand::SetModel { model } => {
                let listings = self.handle.list_model_listings().await;
                let model_selection = model.into_projection();
                let (source, route_hint) = split_model_selection_source(model_selection, &listings);
                match self
                    .handle
                    .select_model(lingxi_core::host::ModelSelectionRequest::Select {
                        source,
                        route_hint,
                        origin: lingxi_core::host::ModelSelectionOrigin::Sdk,
                        validation:
                            lingxi_core::host::ModelValidationMode::GzrAbortAfterFiveSeconds,
                    })
                    .await
                {
                    Ok(()) => {
                        let current = self.handle.get_status_snapshot().await;
                        let _ = self.refresh_media_cloud_audio_binding().await;
                        let selected = lingxi_core::host::qualified_model_ref(
                            &current.model,
                            current.model_profile.as_deref(),
                        );
                        sink.emit(ClientEvent::ModelChanged {
                            model: selected,
                            model_source: current.model_source.as_ref().and_then(|source| {
                                client::protocol::text::ClientTextDto::from_projection(source).ok()
                            }),
                        })
                        .await;
                        if self.persisted_reasoning_selection().is_some() {
                            if self.restore_persisted_reasoning_selection().await.is_err() {
                                let _ = self
                                    .handle
                                    .set_reasoning_selection(
                                        lingxi_core::host::ReasoningSelection::Automatic,
                                    )
                                    .await;
                            }
                        } else if let Some(controls) = self.handle.conversation_controls().await {
                            if controls.requested_reasoning_selection
                                != lingxi_core::host::ReasoningSelection::Automatic
                                && controls.requested_reasoning_selection
                                    != controls.effective_reasoning_selection
                            {
                                let _ = self
                                    .handle
                                    .set_reasoning_selection(
                                        lingxi_core::host::ReasoningSelection::Automatic,
                                    )
                                    .await;
                            }
                        }
                        self.emit_controls_snapshot(&*sink).await;
                    }
                    Err(e) => {
                        sink.emit(ClientEvent::Error {
                            kind: ErrorKindDto::Internal,
                            message: format!("switch_model failed: {e}").into(),
                        })
                        .await;
                    }
                }
            }
            ClientCommand::RefreshProviderCatalog { trigger } => {
                if let Some(source) = &self.official_catalog {
                    match trigger.parse() {
                        Ok(trigger) => {
                            if let Err(error) = source.trigger(trigger).await {
                                tracing::warn!(%error, "official provider directory refresh failed");
                            }
                        }
                        Err(error) => {
                            sink.emit(ClientEvent::Error {
                                kind: ErrorKindDto::Internal,
                                message: error.into(),
                            })
                            .await;
                        }
                    }
                }
                self.emit_listing(ListingKindDto::Models, &*sink).await;
            }
            ClientCommand::ListModels => {
                self.emit_listing(ListingKindDto::Models, &*sink).await;
            }
            ClientCommand::GetConversationControls => {
                self.emit_controls_snapshot(&*sink).await;
            }
            ClientCommand::SetReasoningSelection { selection } => {
                let requested = decode_reasoning_selection(selection);
                let previous = self.handle.conversation_controls().await.map(|controls| {
                    (
                        controls.requested_reasoning_selection,
                        controls.effective_reasoning_selection,
                        controls.reasoning_spec.selections_persistable,
                    )
                });
                if let Err(error) = self.handle.set_reasoning_selection(requested).await {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Rejected,
                        message: format!("set_reasoning_selection failed: {error}").into(),
                    })
                    .await;
                    return;
                }
                let (effective, persistable) = self
                    .handle
                    .conversation_controls()
                    .await
                    .map(|controls| {
                        (
                            controls.effective_reasoning_selection,
                            controls.reasoning_spec.selections_persistable,
                        )
                    })
                    .unwrap_or((lingxi_core::host::ReasoningSelection::Automatic, true));
                let persisted_default = if persistable {
                    effective
                } else {
                    lingxi_core::host::ReasoningSelection::Automatic
                };
                if let Err(error) = self.persist_reasoning_selection(&persisted_default) {
                    let rollback = previous
                        .as_ref()
                        .map(|(requested, _, _)| requested.clone())
                        .unwrap_or(lingxi_core::host::ReasoningSelection::Automatic);
                    let _ = self.handle.set_reasoning_selection(rollback).await;
                    let previous_default = previous.as_ref().map_or(
                        lingxi_core::host::ReasoningSelection::Automatic,
                        |(_, effective, persistable)| {
                            if *persistable {
                                effective.clone()
                            } else {
                                lingxi_core::host::ReasoningSelection::Automatic
                            }
                        },
                    );
                    let _ = self.persist_reasoning_selection(&previous_default);
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Rejected,
                        message: format!("persist reasoning selection failed: {error}").into(),
                    })
                    .await;
                }
                self.emit_controls_snapshot(&*sink).await;
            }
            ClientCommand::SetFastMode { enabled } => {
                if let Err(error) = self.handle.set_fast_mode(enabled).await {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Rejected,
                        message: format!("set_fast_mode failed: {error}").into(),
                    })
                    .await;
                    return;
                }
                sink.emit(ClientEvent::FastModeChanged {
                    enabled: self.handle.fast_mode().await,
                })
                .await;
                self.emit_controls_snapshot(&*sink).await;
            }

            // ── Slash commands ──────────────────────────────────────────────
            ClientCommand::RunSlashCommand { turn_id: _, .. } => {
                // Slash expansion is admitted by BridgeConnection, which owns
                // the cancellation token and the active-turn identity. A router
                // fallback has neither authority and must never expand work.
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Rejected,
                    message: "slash command requires connection-owned admission".into(),
                })
                .await;
            }

            // ── Listings ─────────────────────────────────────────────────────
            ClientCommand::RefreshListings { which } => {
                for kind in which {
                    self.emit_listing(kind, &*sink).await;
                }
            }
            ClientCommand::ListSessionAgents => {
                self.emit_session_agent_list(&*sink).await;
            }
            ClientCommand::LoadSessionAgentTranscript { agent_id } => {
                self.emit_session_agent_transcript(agent_id, &*sink).await;
            }

            // ── Settings ─────────────────────────────────────────────────────
            ClientCommand::UpdatePromptSuggestionContext {
                session_id,
                focus,
                input_empty,
                pending_permission,
                elicitation_active,
            } => {
                let _ = self
                    .handle
                    .update_prompt_suggestion_context(
                        lingxi_core::host::orchestrator::PromptSuggestionContext {
                            session_id,
                            focus,
                            input_empty,
                            pending_permission,
                            elicitation_active,
                        },
                    )
                    .await;
            }
            ClientCommand::RecordPromptSuggestionSubmission { text } => {
                let _ = self.handle.record_prompt_suggestion_submission(&text).await;
            }
            ClientCommand::PromptSuggestionFeedback {
                session_id,
                uuid,
                action,
            } => {
                let _ = self
                    .handle
                    .prompt_suggestion_feedback(&session_id, &uuid, &action)
                    .await;
            }
            ClientCommand::UpdateSettings {
                destination,
                patch_json,
            } => {
                self.apply_settings_patch(destination, &patch_json, &*sink)
                    .await;
            }

            // ── Permissions (persisted) ─────────────────────────────────────
            ClientCommand::UpdatePermissionRules {
                destination,
                behavior,
                add,
                remove,
            } => {
                self.apply_permission_rule_update(destination, behavior, add, remove, &*sink)
                    .await;
            }
            ClientCommand::SetDefaultPermissionMode { destination, mode } => {
                self.apply_default_permission_mode(destination, mode, &*sink)
                    .await;
            }
            ClientCommand::UpdateWorkspaceDirectories {
                destination,
                add,
                remove,
            } => {
                self.apply_workspace_directories_update(destination, add, remove, &*sink)
                    .await;
            }

            // ── MCP servers (persisted) ──────────────────────────────────────
            ClientCommand::UpsertMcpServer {
                scope,
                name,
                config_json,
            } => {
                self.apply_mcp_upsert(scope, &name, &config_json, &*sink)
                    .await;
            }
            ClientCommand::RemoveMcpServer { scope, name } => {
                self.apply_mcp_remove(scope, &name, &*sink).await;
            }
            ClientCommand::SkillAdmin { command } => {
                self.apply_skill_admin(command, &*sink).await;
            }
            ClientCommand::McpAdmin { command } => {
                self.apply_mcp_admin(command, &*sink).await;
            }
            ClientCommand::FusionAdmin { command } => {
                self.apply_fusion_admin(command, &*sink).await;
            }
            ClientCommand::PluginAdmin { command } => {
                self.apply_plugin_admin(command, &*sink).await;
            }
            ClientCommand::HookAdmin { command } => {
                self.apply_hook_admin(command, &*sink).await;
            }

            // ── Auth ───────────────────────────────────────────────────────
            // This is a credential write: the Anthropic OAuth handle persists
            // tokens on success. The shared AuthHandle is wrapped by
            // `FusionCatalogClearingAuth`, which cheaply publishes the canonical
            // `anthropic-oauth` route before returning. Do not repeat the global
            // refresh here; that would create a second reconciliation owner.
            ClientCommand::Login => {
                let state = match self.auth.login().await {
                    Ok(li) => lower_auth_state(Some(li)),
                    Err(e) => {
                        sink.emit(ClientEvent::Error {
                            kind: ErrorKindDto::Internal,
                            message: format!("login failed: {e}").into(),
                        })
                        .await;
                        lower_auth_state(self.auth.current_user().await)
                    }
                };
                sink.emit(ClientEvent::AuthState { state }).await;
            }
            // Round-12 finding [2], class member the finding itself did not
            // name: signing OUT is a credential removal too, and
            // `refresh_inner`'s closing `guard.entry("anthropic")
            // .or_insert(..)` is an `or_insert`, so an `anthropic: true` that
            // a sign-in published survives a sign-out for the life of the
            // process. Deliberately NOT fixed with a call here: `self.auth`
            // is the SAME `Arc<dyn AuthHandle>` the TUI's `/logout`
            // (`command_api::builtins::LogoutHandler`) drives, so the clearing lives
            // in `harness_runtime::desktop::FusionCatalogClearingAuth`, which wraps
            // that one handle for every sign-out surface in the process at
            // once. Adding a second call here would double-fire the same
            // fan-out and let the two mechanisms drift.
            ClientCommand::Logout => {
                if let Err(e) = self.auth.logout().await {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Internal,
                        message: format!("logout failed: {e}").into(),
                    })
                    .await;
                }
                sink.emit(ClientEvent::AuthState {
                    state: lower_auth_state(self.auth.current_user().await),
                })
                .await;
            }

            // ── Compaction ───────────────────────────────────────────────────
            ClientCommand::ForceCompact => match self.handle.force_compact().await {
                Ok(summary) => {
                    sink.emit(ClientEvent::CompactionCompleted {
                        messages_before: summary.messages_before,
                        messages_after: summary.messages_after,
                        bytes_saved: summary.bytes_saved,
                        summary: client::protocol::text::ClientTextDto::from_projection(
                            &summary.summary,
                        )
                        .expect("compaction summary is text"),
                    })
                    .await;
                }
                Err(e) => {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Internal,
                        message: format!("force_compact failed: {e}").into(),
                    })
                    .await;
                }
            },

            // ── Session control ──────────────────────────────────────────────
            ClientCommand::ClearSession => {
                // Mid-turn semantics (plan §2): reject `ClearSession` while a
                // turn is in flight — it must NOT reach the engine.
                if self.is_turn_active() {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "cannot clear the session while a turn is in flight".into(),
                    })
                    .await;
                    return;
                }
                let previous_session_id = self.handle.current_session_id().await;
                match self.handle.clear_session().await {
                    Ok(()) => {
                        if let Some(workspace) = &self.workspace_commands {
                            workspace.session_facts().reset(false);
                        }
                        let current_session_id = self.handle.current_session_id().await;
                        let presence_warning = self
                            .refresh_process_session_presence(
                                previous_session_id,
                                current_session_id,
                            )
                            .await;
                        sink.emit(ClientEvent::SessionEnded).await;
                        if let Some(message) = presence_warning {
                            sink.emit(ClientEvent::SystemNotice {
                                message: message.into(),
                                is_error: false,
                            })
                            .await;
                        }
                    }
                    Err(e) => {
                        sink.emit(ClientEvent::Error {
                            kind: ErrorKindDto::Internal,
                            message: format!("clear_session failed: {e}").into(),
                        })
                        .await;
                    }
                }
            }
            ClientCommand::ListSessions { limit } => {
                let limit = limit.map_or(DEFAULT_SESSION_LIST_LIMIT, |value| value as usize);
                self.emit_session_list(limit, &*sink).await;
            }
            ClientCommand::NewSession { cwd: _, model } => {
                if self.is_turn_active() {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "cannot start a new session while a turn is in flight".into(),
                    })
                    .await;
                    return;
                }

                let requested_model = match model {
                    Some(model) => {
                        let listings = self.handle.list_model_listings().await;
                        let (source, route_hint) =
                            split_model_selection_source(model.into_projection(), &listings);
                        Some(lingxi_core::host::ModelSelectionRequest::Select {
                            source,
                            route_hint,
                            origin: lingxi_core::host::ModelSelectionOrigin::Sdk,
                            validation:
                                lingxi_core::host::ModelValidationMode::GzrAbortAfterFiveSeconds,
                        })
                    }
                    None => None,
                };
                if let Some(request) = requested_model.as_ref() {
                    if let Err(error) = self.handle.validate_model_selection(request).await {
                        sink.emit(ClientEvent::Error {
                            kind: ErrorKindDto::Internal,
                            message: format!("new session model validation failed: {error}").into(),
                        })
                        .await;
                        return;
                    }
                }

                let previous_session_id = self.handle.current_session_id().await;
                if let Err(error) = self.handle.clear_session().await {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Internal,
                        message: format!("new session (clear_session) failed: {error}").into(),
                    })
                    .await;
                    return;
                }
                if let Some(workspace) = &self.workspace_commands {
                    workspace.session_facts().reset(false);
                }
                let current_session_id = self.handle.current_session_id().await;
                let presence_warning = self
                    .refresh_process_session_presence(previous_session_id, current_session_id)
                    .await;

                if let Some(request) = requested_model {
                    if let Err(error) = self.handle.select_model(request).await {
                        sink.emit(ClientEvent::Error {
                            kind: ErrorKindDto::Internal,
                            message: format!("new session model switch failed: {error}").into(),
                        })
                        .await;
                        return;
                    }
                }
                let _ = self.restore_persisted_reasoning_selection().await;

                let _ = self.refresh_media_cloud_audio_binding().await;
                let session_id = self.handle.current_session_id().await.to_string();
                sink.emit(ClientEvent::SessionStarted {
                    session_id,
                    mode: SessionModeDto::Code,
                })
                .await;
                if let Some(message) = presence_warning {
                    sink.emit(ClientEvent::SystemNotice {
                        message: message.into(),
                        is_error: false,
                    })
                    .await;
                }
            }
            ClientCommand::ResumeSession { session_id, cwd } => {
                if self.is_turn_active() {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "cannot resume while a turn is in flight".into(),
                    })
                    .await;
                    return;
                }

                let uuid = match uuid::Uuid::parse_str(&session_id) {
                    Ok(uuid) => uuid,
                    Err(error) => {
                        sink.emit(ClientEvent::Error {
                            kind: ErrorKindDto::Protocol,
                            message: format!(
                                "resume: malformed session id {session_id:?}: {error}"
                            )
                            .into(),
                        })
                        .await;
                        return;
                    }
                };

                let Some(store) = self.session_store.as_ref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Internal,
                        message: "resume: persisted session store is unavailable".into(),
                    })
                    .await;
                    return;
                };
                let cwd = cwd.unwrap_or_else(|| store.current_cwd());
                let replayed = match orchestrator::replay_session_state(
                    &store.lingxi_home,
                    &cwd,
                    uuid,
                    store.fs.clone(),
                )
                .await
                {
                    Ok(replayed) => replayed,
                    Err(error) => {
                        sink.emit(ClientEvent::Error {
                            kind: ErrorKindDto::Protocol,
                            message: format!("resume: session {session_id} not resumable: {error}")
                                .into(),
                        })
                        .await;
                        return;
                    }
                };

                if let Err(message) = self
                    .resume_replayed_session(uuid, replayed, sink.clone())
                    .await
                {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Internal,
                        message: message.into(),
                    })
                    .await;
                }
            }
            ClientCommand::RequestExit => {
                self.handle.request_exit().await;
            }

            // ── Tasks ──────────────────────────────────────────────────────
            ClientCommand::TaskList {
                status_filter,
                request_id,
            } => {
                let filter = TaskListFilter {
                    status: status_filter.map(task_status_wire),
                };
                if let Some(request_id) = request_id {
                    emit_correlated_task_list(&*self.tasks, &*sink, filter, request_id).await;
                } else {
                    self.emit_task_list(filter, &*sink).await;
                }
            }
            ClientCommand::TaskOutput { task_id, offset } => {
                match self.tasks.output(&task_id, Some(offset)).await {
                    Ok(chunk) => {
                        let (id, content, total_lines, truncated) = lower_task_output_chunk(&chunk);
                        sink.emit(ClientEvent::TaskOutputChunk {
                            task_id: id,
                            content,
                            total_lines,
                            truncated,
                        })
                        .await;
                    }
                    Err(e) => {
                        sink.emit(ClientEvent::Error {
                            kind: ErrorKindDto::Internal,
                            message: format!("task output failed: {e}").into(),
                        })
                        .await;
                    }
                }
            }
            // A stop from the desktop client is the USER's — claude-code's
            // UI/control-channel `stopTask` passes `source:"user"` and inherits
            // the stop helper's `killedBy = "user"` default, so the killed
            // notification reads "was stopped by user".
            ClientCommand::TaskMessage { task_id, message } => {
                if !self.handle.workspace_trusted().await {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Rejected,
                        message: "Trust this workspace before messaging a task".into(),
                    })
                    .await;
                } else {
                    let message =
                        lingxi_core::types::utf16_json::Utf16JsonProjection::from(message);
                    match self.tasks.send_human_task_message(&task_id, &message).await {
                        Ok(()) => {
                            sink.emit(ClientEvent::SystemNotice {
                                message: format!("Message accepted for task {task_id}").into(),
                                is_error: false,
                            })
                            .await
                        }
                        Err(error) => {
                            sink.emit(ClientEvent::Error {
                                kind: ErrorKindDto::Rejected,
                                message: format!("task message failed: {error}").into(),
                            })
                            .await
                        }
                    }
                }
            }
            ClientCommand::TaskStop { task_id } => {
                match self.tasks.kill_with_reason(&task_id, "user").await {
                    Ok(rec) => {
                        sink.emit(ClientEvent::TaskStatusChanged {
                            task_id: rec.task_id,
                            status: client::adapter::lowering::lower_task_status(&rec.status),
                            origin_session_id: None,
                            // A user stop is `killed`, never `failed` — no
                            // handler-reported reason to forward.
                            error: None,
                        })
                        .await;
                    }
                    Err(e) => {
                        sink.emit(ClientEvent::Error {
                            kind: ErrorKindDto::Internal,
                            message: format!("task stop failed: {e}").into(),
                        })
                        .await;
                    }
                }
            }

            // Workflow resume is a mobile host capability. The
            // desktop bridge has no workflow launcher bound into this router;
            // surface that fact as a protocol error instead of silently
            // dropping the command in the catch-all below.
            ClientCommand::ResumeWorkflow { .. } => {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Rejected,
                    message: "resume_workflow is unavailable on this bridge"
                        .to_string()
                        .into(),
                })
                .await;
            }

            // Durable turn recovery and queue inspection are MOBILE host
            // capabilities: they need the retained
            // per-turn event log and the recovery state machine that
            // `harness-runtime::mobile`'s host owns (`host.rs`'s `AttachTurn` /
            // `ResumeTurn` / `PauseTurn` arms). This bridge keeps no retained
            // event window and no `TurnRecoverySnapshotDto` state, so there is
            // nothing here to attach to, resume, or pause.
            //
            // Each is a REQUEST-REPLY command on the mobile host —
            // the client sends it and waits for a `turn_recovery_state` (and,
            // for `attach_turn`, a `turn_event_replay` burst). Dropping them in
            // the catch-all below leaves such a client waiting forever for a
            // reply that will never come, so answer with the same explicit
            // typed rejection `ResumeWorkflow` uses. One arm per command so the
            // message names the command the client actually sent.
            ClientCommand::AttachTurn { .. } => {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Rejected,
                    message: "attach_turn is unavailable on this bridge"
                        .to_string()
                        .into(),
                })
                .await;
            }
            ClientCommand::ResumeTurn { .. } => {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Rejected,
                    message: "resume_turn is unavailable on this bridge"
                        .to_string()
                        .into(),
                })
                .await;
            }
            ClientCommand::PauseTurn { .. } => {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Rejected,
                    message: "pause_turn is unavailable on this bridge"
                        .to_string()
                        .into(),
                })
                .await;
            }

            // ── Handled elsewhere / not routed by this seam ──────────────────
            //
            // `SendPrompt` + `Cancel` are the turn path (`TurnDriver`), and
            // `ApprovePermission`/`DenyPermission` the permission path — both on
            // `BridgeConnection` directly.
            // The `#[non_exhaustive]` enum also requires a catch-all.
            other => {
                tracing::debug!(
                    ?other,
                    "bridge-server: command not routed by EngineCommandRouter"
                );
            }
        }
    }
}

pub(crate) fn requires_idle_turn_for_slash(
    registry: &CommandRegistry,
    raw: &Utf16JsonProjection,
) -> bool {
    let Some(parsed) = parse_slash_command(raw) else {
        return false;
    };
    let Some(command) = registry.resolve(&parsed.name) else {
        return false;
    };
    // The current registry resolves declared aliases (for example `new` and
    // `reset`) to the canonical `/clear` builtin. Check the resolved source and
    // name rather than comparing user text, so custom commands and unknown or
    // case-mismatched spellings keep their ordinary dispatch behavior.
    command.source == CommandSource::Builtin
        && (source_text_eq_ascii(&command.name, "clear")
            || source_text_eq_ascii(&command.name, "compact")
            || source_text_eq_ascii(&command.name, "cd"))
}

/// Serve the access token of the Codex session the desktop sent. Returns the
/// failure to report, if any; the session never appears in it.
fn serve_openai_host_token(
    session: &llm_runtime::auth::openai::HostSession,
    credential: &str,
) -> Option<String> {
    let delivered = (credential.len() <= crate::boot::MAX_STDIN_CREDENTIAL_BYTES)
        .then(|| {
            serde_json::from_str::<client::protocol::events::OpenAiOAuthSessionDto>(credential).ok()
        })
        .flatten()
        .filter(|delivered| crate::boot::validate_openai_oauth_session(delivered).is_ok());
    let Some(delivered) = delivered else {
        return Some("invalid Codex OAuth session".to_string());
    };
    session.serve(crate::boot::openai_host_token(&delivered));
    None
}

fn provider_id_is_valid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.chars().enumerate().all(|(index, ch)| {
            ch.is_ascii_lowercase()
                || ch.is_ascii_digit()
                || (index > 0 && matches!(ch, '-' | '_' | '.'))
        })
}

fn command_source_string(source: CommandSource) -> &'static str {
    match source {
        CommandSource::Builtin => "builtin",
        CommandSource::Settings(lingxi_core::types::SettingsScope::User) => "user",
        CommandSource::Settings(lingxi_core::types::SettingsScope::Project) => "project",
        CommandSource::Settings(lingxi_core::types::SettingsScope::Local) => "local",
        CommandSource::Plugin => "plugin",
        CommandSource::Settings(lingxi_core::types::SettingsScope::Managed) => "managed",
        CommandSource::Mcp => "mcp",
        CommandSource::Bundled => "bundled",
    }
}

#[cfg(test)]
#[path = "router/tests/settings_patch_parsing_tests.rs"]
mod settings_patch_parsing_tests;

#[cfg(test)]
#[path = "router/tests/mcp_config_json_parsing_tests.rs"]
mod mcp_config_json_parsing_tests;

/// Round-9 review finding [2]: the Electron desktop's ONLY credential-add path
/// is [`ClientCommand::SetProviderCredential`] (Settings -> Provider
/// Credentials). It must tell Fusion's scoped catalog refresher about the
/// write, exactly the way the TUI key view does
/// (`apps/cli/src/mode.rs`'s `run_connect_action`); without it a key added in
/// Settings routes on the next ordinary turn but stays invisible to `/fusion`
/// for the rest of the engine process.
#[cfg(test)]
#[path = "router/tests/fusion_catalog_refresh_tests.rs"]
mod fusion_catalog_refresh_tests;

#[cfg(test)]
#[path = "router/tests/scheduled_chat_anchor_tests.rs"]
mod scheduled_chat_anchor_tests;

#[cfg(test)]
#[path = "router/tests/runtime_snapshot_tests.rs"]
mod runtime_snapshot_tests;

#[cfg(test)]
#[path = "router/tests/side_chat_shutdown_tests.rs"]
mod side_chat_shutdown_tests;

mod catalog;
mod configuration;
mod sessions;
mod settings;
mod tasks;

#[cfg(test)]
use settings::parse_settings_patch;
use tasks::emit_correlated_task_list;
use tasks::task_status_wire;
pub use tasks::TaskPoll;

#[cfg(test)]
use configuration::parse_mcp_config_json;
