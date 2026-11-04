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
//! ([`lingxi_core::host::OrchestratorHandle`],
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
use client::protocol::listings::SessionModeDto;
use client::protocol::text::ClientTextDto;
use command_api::model::CommandSource;
use command_api::parser::parse_slash_command;
use command_api::registry::CommandRegistry;
use lingxi_core::host::orchestrator::HandleError;
use lingxi_core::host::orchestrator::OrchestratorHandle;
use lingxi_core::host::session_context::ConversationMode;
use lingxi_core::host::task_registry::{TaskListFilter, TaskRegistryHandle};
use lingxi_core::host::SlashCommandDispatcher;
use lingxi_core::types::utf16_json::Utf16JsonProjection;
use media_api::MediaOwner;
use media_runtime::{MediaAttachmentHandle, MediaAttachmentPhase};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::mcp_bridge::McpPaths;
use crate::settings_bridge::SettingsContext;

fn lower_conversation_mode(mode: ConversationMode) -> SessionModeDto {
    match mode {
        ConversationMode::Chat => SessionModeDto::Chat,
        ConversationMode::Code => SessionModeDto::Code,
    }
}

fn conversation_mode_from_dto(mode: SessionModeDto) -> ConversationMode {
    match mode {
        SessionModeDto::Chat => ConversationMode::Chat,
        SessionModeDto::Code => ConversationMode::Code,
    }
}

/// Match the client-visible profile to the selected account identity. The
/// registry retains the logical pool profile so later admissions can verify
/// it; a concrete account scope must travel with its concrete profile.
fn audio_context_profile(logical_profile: String, scope: &str) -> Option<String> {
    if scope == lingxi_core::host::provider_accounts::UNBOUND_ACCOUNT_SCOPE
        || scope.starts_with("explicit-service.")
    {
        return Some(logical_profile);
    }
    let identity =
        serde_json::from_str::<llm_runtime::accounts::AccountResourceScope>(scope).ok()?;
    if identity.account_id.is_empty() || identity.store_scope.is_empty() {
        return None;
    }
    Some(format!("account.{}", identity.account_id))
}

#[cfg(test)]
mod audio_context_profile_tests {
    use super::audio_context_profile;

    #[test]
    fn selected_account_scope_uses_its_concrete_profile() {
        let account = serde_json::to_string(&llm_runtime::accounts::AccountResourceScope {
            store_scope: "domain".into(),
            account_id: "selected".into(),
            identity_generation: 7,
        })
        .unwrap();
        assert_eq!(
            audio_context_profile("pool".into(), &account).as_deref(),
            Some("account.selected")
        );
        assert_eq!(
            audio_context_profile(
                "pool".into(),
                lingxi_core::host::provider_accounts::UNBOUND_ACCOUNT_SCOPE
            )
            .as_deref(),
            Some("pool")
        );
        assert_eq!(
            audio_context_profile("pool".into(), "explicit-service.fingerprint").as_deref(),
            Some("pool")
        );
        assert_eq!(audio_context_profile("pool".into(), "profile:pool"), None);
    }
}

fn replayed_conversation_mode(
    replayed: &orchestrator::ReplayedSession,
    session_id: uuid::Uuid,
) -> Result<ConversationMode, String> {
    let source = replayed
        .transcript_metadata
        .iter()
        .map(|row| row.to_json_string().map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()?
        .join("\n");
    session::JsonlWriter::fixed_session_mode_from_source(&source, &session_id.to_string())
}

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

/// A one-use resume captured before the connection drains session resources.
pub struct PreparedSessionTransition {
    id: uuid::Uuid,
    mode: ConversationMode,
    replayed: orchestrator::ReplayedSession,
    source: lingxi_core::host::PinnedResumeSource,
}

struct PendingForkSource {
    identity: lingxi_core::host::PinnedResumeSource,
    content_len: usize,
    content_sha256: [u8; 32],
    // Capacity is reserved before publication and held until the first mount.
    _slot: tokio::sync::OwnedSemaphorePermit,
}

impl PendingForkSource {
    fn matches(&self, source: &lingxi_core::host::PinnedResumeSource) -> bool {
        self.identity.root == source.root
            && self.identity.relative == source.relative
            && self.identity.root_identity == source.root_identity
            && self.identity.file_identity == source.file_identity
            && self.content_len == source.expected_content.len()
            && self.content_sha256
                == remote_control_client::encoding::sha256(source.expected_content.as_bytes())
    }
}

/// Inputs needed to enumerate and replay the persisted JSONL session store.
///
/// Optional for routers without session operations. Production bridge boot
/// supplies the authoritative session roots.
pub struct SessionStoreContext {
    lingxi_home: PathBuf,
    session_cwd: String,
    runtime_cwd: Option<Arc<tool_api::SessionCwd>>,
    fs: Arc<dyn lingxi_core::host::FileSystem>,
    transcript_writer: Option<Arc<session::jsonl::JsonlWriter>>,
}

fn replay_pinned_catalog_source(
    home: &std::path::Path,
    path: &std::path::Path,
    id: uuid::Uuid,
) -> Result<
    (
        orchestrator::ReplayedSession,
        lingxi_core::host::PinnedResumeSource,
    ),
    String,
> {
    let (authority, content) = crate::boot::capture_selected_transcript_source(home, path, id)?;
    replay_captured_authority(authority, content, id)
}

fn replay_captured_authority(
    authority: session::jsonl::writer::TranscriptFileAuthority,
    content: String,
    id: uuid::Uuid,
) -> Result<
    (
        orchestrator::ReplayedSession,
        lingxi_core::host::PinnedResumeSource,
    ),
    String,
> {
    let path = authority.path();
    let loaded = session::jsonl::loader::route_captured_session_source(&path, id, &content)
        .map_err(|error| format!("resume: selected session is not resumable: {error}"))?;
    crate::boot::validate_captured_transcript(&path, id, &content, &loaded, false)?;
    let replayed = orchestrator::resume::replay_captured_session(path, content.clone(), loaded, id)
        .map_err(|error| format!("resume: selected session is not resumable: {error}"))?;
    let source = authority.pinned_resume_source(content);
    Ok((replayed, source))
}

async fn replay_unique_catalog_source(
    home: &std::path::Path,
    id: uuid::Uuid,
) -> Result<
    (
        orchestrator::ReplayedSession,
        lingxi_core::host::PinnedResumeSource,
    ),
    String,
> {
    let (_, authority) =
        harness_runtime::headless::session::resolve_unique_resume_source(home, id).await?;
    let content = String::from_utf8(
        authority
            .read_checked()
            .map_err(|error| error.to_string())?,
    )
    .map_err(|_| "resume: transcript is not valid UTF-8")?;
    replay_captured_authority(authority, content, id)
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
    async fn ensure_scheduled_chat(
        &self,
        session_id: &str,
        mode: ConversationMode,
        cwd: &std::path::Path,
        title: &str,
    ) -> Result<(), String> {
        let session_id = lingxi_core::types::SessionId::parse_prefixed(session_id)
            .ok_or("Invalid scheduled task chat identity")?;
        let writer = self
            .transcript_writer
            .as_ref()
            .ok_or("Scheduled task chat storage is unavailable")?;
        writer
            .ensure_draft_anchor(session_id, mode, cwd, title)
            .await
            .map(|_| ())
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
    /// Validate a proposed replacement before the connection pauses any
    /// session-owned resources. This query must not mutate the runtime.
    async fn preflight_session_transition(
        &self,
        _command: &ClientCommand,
    ) -> Result<Option<PreparedSessionTransition>, (ErrorKindDto, String)> {
        Ok(None)
    }
    /// Consume the exact preflight result, if any, for this transition.
    async fn route_prepared_session_transition(
        &self,
        command: ClientCommand,
        _prepared: Option<PreparedSessionTransition>,
        sink: Arc<dyn ClientEventSink>,
    ) {
        self.route(command, sink).await;
    }
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

    /// Update metadata on the active transcript through its writer authority.
    async fn session_metadata(
        &self,
        _request: Utf16JsonProjection,
    ) -> Result<Utf16JsonProjection, String> {
        Err("session metadata is unavailable".into())
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
    handoff_manager: Arc<harness_runtime::handoff::SessionHandoffManager>,
    session_cron: Option<Arc<cron::CronScheduler>>,
    cron_firer: Option<Arc<crate::cron_host::HostCronFirer>>,
    handle: Arc<dyn OrchestratorHandle>,
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
    /// Exact published targets awaiting their first in-process fork resume.
    /// A process restart turns a later open into a new catalog selection.
    pending_fork_sources: std::sync::Mutex<HashMap<uuid::Uuid, Arc<PendingForkSource>>>,
    pending_fork_capacity: Arc<tokio::sync::Semaphore>,
    /// Process-owned observer facts supplement the live task registry for
    /// foreground children and allocation before the first transcript write.
    session_agent_observer:
        Option<Arc<harness_runtime::desktop::session_agents::DesktopSessionAgentObserver>>,
    /// Current coordinator ownership supplements persisted agent transcripts
    /// when a host reconnects after missing background completion events.
    team_registry: Option<Arc<dyn lingxi_core::host::team_registry::TeamRegistryHandle>>,
    /// Shared runtime catalog used by account-aware model routes.
    catalog_registry: harness_runtime::desktop::FusionCatalogRegistry,
    provider_account_store: Option<Arc<harness_runtime::provider_accounts::ProviderAccountStore>>,
    provider_account_update_lock:
        tokio::sync::Mutex<Option<lingxi_core::host::provider_accounts::AccountSnapshot>>,
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
        let created = harness_runtime::side_chat::SideChatManager::new_recoverable(
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

    async fn replay_resume_source(
        &self,
        home: &std::path::Path,
        uuid: uuid::Uuid,
    ) -> Result<
        (
            orchestrator::ReplayedSession,
            lingxi_core::host::PinnedResumeSource,
        ),
        String,
    > {
        let pending = self
            .pending_fork_sources
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(&uuid)
            .cloned();
        let Some(pending) = pending else {
            return replay_unique_catalog_source(home, uuid).await;
        };
        let authority = session::jsonl::writer::TranscriptFileAuthority::from_capture(
            pending.identity.root.clone(),
            pending.identity.relative.clone(),
            pending.identity.root_identity,
            pending.identity.file_identity,
        )
        .map_err(|error| format!("resume: invalid fork target authority: {error}"))?;
        let content = authority
            .read_checked()
            .map_err(|error| format!("resume: published fork target changed: {error}"))?;
        if content.len() != pending.content_len
            || remote_control_client::encoding::sha256(&content) != pending.content_sha256
        {
            return Err("resume: published fork target changed".into());
        }
        authority
            .verify_current()
            .map_err(|error| format!("resume: published fork target changed: {error}"))?;
        let content = String::from_utf8(content)
            .map_err(|_| "resume: published fork target is not valid UTF-8")?;
        replay_captured_authority(authority, content, uuid)
    }

    async fn resume_replayed_session(
        &self,
        uuid: uuid::Uuid,
        mut replayed: orchestrator::ReplayedSession,
        expected_mode: ConversationMode,
        source: lingxi_core::host::PinnedResumeSource,
        sink: Arc<dyn ClientEventSink>,
    ) -> Result<(), String> {
        let pinned_fork_source = self
            .pending_fork_sources
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(&uuid)
            .cloned();
        if pinned_fork_source
            .as_ref()
            .is_some_and(|expected| !expected.matches(&source))
        {
            return Err("resume: published fork target authority changed".into());
        }
        let authority = session::jsonl::writer::TranscriptFileAuthority::from_capture(
            source.root.clone(),
            source.relative.clone(),
            source.root_identity,
            source.file_identity,
        )
        .map_err(|error| format!("resume: invalid source authority: {error}"))?;
        if authority
            .read_checked()
            .map_err(|error| format!("resume: selected source changed: {error}"))?
            != source.expected_content.as_bytes()
        {
            return Err("resume: selected source changed after admission".into());
        }
        let resumed_mode = replayed_conversation_mode(&replayed, uuid)?;
        if resumed_mode != expected_mode
            || self.handle.conversation_mode().await.mode != expected_mode
        {
            return Err(
                "session_mode_mismatch: expected mode does not match the session and runtime"
                    .into(),
            );
        }
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
        let mut runtime_snapshot = replayed.handle_runtime_snapshot();
        runtime_snapshot.transcript_source = Some(source);
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
        if let Some(expected) = pinned_fork_source {
            let mut pending = self
                .pending_fork_sources
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            if pending
                .get(&uuid)
                .is_some_and(|current| Arc::ptr_eq(current, &expected))
            {
                pending.remove(&uuid);
            }
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

    async fn fork_session(
        &self,
        source_id: uuid::Uuid,
        target_mode: ConversationMode,
    ) -> Result<uuid::Uuid, String> {
        self.fork_session_after_capture(source_id, target_mode, || {})
            .await
    }

    async fn fork_session_after_capture(
        &self,
        source_id: uuid::Uuid,
        target_mode: ConversationMode,
        after_capture: impl FnOnce() + Send,
    ) -> Result<uuid::Uuid, String> {
        self.fork_session_with_capture_hooks(source_id, target_mode, after_capture, || {})
            .await
    }

    async fn fork_session_with_capture_hooks(
        &self,
        source_id: uuid::Uuid,
        target_mode: ConversationMode,
        after_capture: impl FnOnce() + Send,
        after_snapshot: impl FnOnce() + Send,
    ) -> Result<uuid::Uuid, String> {
        if self.is_turn_active() {
            return Err("cannot fork a session while a turn is in flight".into());
        }
        let slot = self
            .pending_fork_capacity
            .clone()
            .try_acquire_owned()
            .map_err(|_| "fork: too many pending fork targets")?;
        let store = self
            .session_store
            .as_ref()
            .ok_or("fork: persisted session store is unavailable")?;
        let cwd = store.current_cwd();
        let current = self.handle.current_session_id().await.as_uuid();
        let active_writer = if source_id == current {
            Some(
                store
                    .transcript_writer
                    .as_ref()
                    .ok_or("fork: active writer authority is unavailable")?,
            )
        } else {
            None
        };
        let (authority, source_bytes, content) = if let Some(writer) = active_writer {
            let authority = writer
                .active_transcript_authority()
                .ok_or("fork: active transcript authority is unavailable")?;
            let capture = self
                .workspace_commands
                .as_ref()
                .ok_or("fork: active writer authority is unavailable")?
                .capture_current_transcript(lingxi_core::types::SessionId::from_uuid(source_id))
                .await?;
            writer
                .verify_active_transcript_source(&authority)
                .map_err(|error| format!("fork: {error}"))?;
            if capture.transcript_path != authority.path() {
                return Err("fork: active transcript changed during capture".into());
            }
            let bytes = authority
                .read_checked()
                .map_err(|error| format!("fork: {error}"))?;
            // The committed capture excludes a torn final line. Retain the
            // full pinned bytes for identity/content checks while branching
            // only the writer's complete, committed prefix.
            if bytes
                .strip_prefix(capture.source.as_bytes())
                .is_none_or(|tail| tail.contains(&b'\n'))
            {
                return Err("fork: active transcript changed during capture".into());
            }
            (authority, bytes, capture.source)
        } else {
            let (_, authority) = harness_runtime::headless::session::resolve_unique_resume_source(
                &store.lingxi_home,
                source_id,
            )
            .await
            .map_err(|error| format!("fork: {error}"))?;
            let bytes = authority
                .read_checked()
                .map_err(|error| format!("fork: {error}"))?;
            let content = String::from_utf8(bytes.clone())
                .map_err(|error| format!("fork: invalid source encoding: {error}"))?;
            (authority, bytes, content)
        };
        let path = authority.path();
        let loaded =
            session::jsonl::loader::route_captured_session_source(&path, source_id, &content)
                .map_err(|error| format!("fork: {error}"))?;
        crate::boot::validate_captured_transcript(&path, source_id, &content, &loaded, false)
            .map_err(|error| format!("fork: {error}"))?;
        let source_mode =
            session::JsonlWriter::fixed_session_mode_from_source(&content, &source_id.to_string())
                .map_err(|error| format!("fork: invalid source mode: {error}"))?;
        if source_mode != target_mode {
            return Err("session_mode_mismatch: fork must preserve the source session mode".into());
        }
        after_capture();
        let live = lingxi_core::host::live_sessions::LiveSessionDir::at_live(
            store.lingxi_home.join("sessions"),
        );
        let source_lease = if source_id == current {
            let lease = self
                .workspace_commands
                .as_ref()
                .ok_or("fork: active writer authority is unavailable")?
                .current_writer_lease(lingxi_core::types::SessionId::from_uuid(source_id))
                .await?;
            if lease.canonical_session_id().map(|id| id.as_uuid()) != Some(source_id) {
                return Err(
                    "fork: active writer authority does not match the source session".into(),
                );
            }
            lease
        } else {
            live.claim_session_id(&source_id.to_string(), std::process::id())
                .map_err(|error| format!("fork: source session is in use: {error}"))?
                .into_shared()
        };
        let verify_source = || -> Result<(), String> {
            if let Some(writer) = active_writer {
                writer
                    .verify_active_transcript_source(&authority)
                    .map_err(|error| format!("fork: {error}"))?;
            }
            if authority
                .read_checked()
                .map_err(|error| format!("fork: {error}"))?
                != source_bytes
            {
                return Err("fork: source transcript changed during capture".into());
            }
            if let Some(writer) = active_writer {
                writer
                    .verify_active_transcript_source(&authority)
                    .map_err(|error| format!("fork: {error}"))?;
            }
            authority
                .verify_current()
                .map_err(|error| format!("fork: {error}"))?;
            Ok(())
        };
        verify_source()?;
        // The claimed, checked snapshot now identifies the fork source. Any
        // later changes to the original file cannot change this branch.
        after_snapshot();
        let target_id = uuid::Uuid::new_v4();
        let _target_lease = live
            .claim_session_id(&target_id.to_string(), std::process::id())
            .map_err(|error| format!("fork: target writer claim failed: {error}"))?
            .into_shared();
        let (messages, _) =
            session::jsonl::build_conversation_chain(&loaded, &source_id.to_string());
        let recovery = harness_runtime::desktop::session_state::capture_branch_recovery(
            &store.lingxi_home,
            source_id,
            source_lease.clone(),
        )
        .await
        .map_err(|error| format!("fork: resume the original session first: {error}"))?;
        orchestrator::native_computer::validate_independent_branch(
            source_id,
            &content,
            &messages,
            recovery.as_ref(),
        )
        .map_err(|error| format!("fork: resume the original session first: {error}"))?;
        let source_cwd = session::jsonl::loader::transcript_workspace_path(&path, &loaded)
            .unwrap_or_else(|| cwd.clone());
        let result = session::branch::create_branch_from_captured(
            session::branch::CapturedBranch {
                lingxi_home: &store.lingxi_home,
                source_cwd: &source_cwd,
                target_cwd: &cwd,
                source_session_id: source_id,
                target_session_id: target_id,
                source_content: &content,
                messages: &messages,
                custom_title: None,
                mode: Some(target_mode),
            },
            store.fs.clone(),
        )
        .await
        .map_err(|error| format!("fork: {error}"))?;
        let target_id = result.result.new_session_id;
        let target_source = Arc::new(PendingForkSource {
            identity: result.authority.pinned_resume_source(String::new()),
            content_len: result.content.len(),
            content_sha256: remote_control_client::encoding::sha256(result.content.as_bytes()),
            _slot: slot,
        });
        let mut pending = self
            .pending_fork_sources
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        match pending.entry(target_id) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(target_source);
            }
            std::collections::hash_map::Entry::Occupied(_) => {
                return Err(format!(
                    "fork: published target {target_id} has a conflicting in-process receipt"
                ));
            }
        }
        Ok(target_id)
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
        let mode = self.handle.conversation_mode().await;
        sink.emit(ClientEvent::SessionResumed {
            session_id: uuid.to_string(),
            mode: lower_conversation_mode(mode.mode),
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
        tasks: Arc<dyn TaskRegistryHandle>,
        dispatcher: Option<Arc<dyn SlashCommandDispatcher>>,
        slash_registry: Option<Arc<RwLock<CommandRegistry>>>,
    ) -> Self {
        Self {
            workspace_commands: None,
            handoff_manager: Arc::new(harness_runtime::handoff::SessionHandoffManager::default()),
            cron_firer: None,
            session_cron: None,
            handle,
            tasks,
            dispatcher,
            slash_registry,
            side_chat_runtime: None,
            side_chat_manager: tokio::sync::Mutex::new(None),
            session_store: None,
            pending_fork_sources: std::sync::Mutex::new(HashMap::new()),
            pending_fork_capacity: Arc::new(tokio::sync::Semaphore::new(1024)),
            session_agent_observer: None,
            team_registry: None,
            catalog_registry: harness_runtime::desktop::FusionCatalogRegistry::default(),
            provider_account_store: None,
            provider_account_update_lock: tokio::sync::Mutex::new(None),
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
                                lingxi_core::host::provider_accounts::UNBOUND_ACCOUNT_SCOPE,
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

    pub fn with_provider_account_store(
        mut self,
        store: Arc<harness_runtime::provider_accounts::ProviderAccountStore>,
        snapshot: Option<lingxi_core::host::provider_accounts::AccountSnapshot>,
    ) -> Self {
        self.provider_account_update_lock = tokio::sync::Mutex::new(snapshot);
        self.provider_account_store = Some(store);
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

    /// Let the parent own account secrets and accept access-only cache updates.
    #[must_use]
    pub fn with_ephemeral_provider_credentials(mut self, enabled: bool) -> Self {
        self.provider_credentials_ephemeral = enabled;
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
    async fn preflight_session_transition(
        &self,
        command: &ClientCommand,
    ) -> Result<Option<PreparedSessionTransition>, (ErrorKindDto, String)> {
        match command {
            ClientCommand::NewSession { mode, .. } => {
                if self.handle.conversation_mode().await.mode != conversation_mode_from_dto(*mode) {
                    return Err((
                        ErrorKindDto::Rejected,
                        "session_mode_mismatch: create this mode in its own runtime".into(),
                    ));
                }
            }
            ClientCommand::ResumeSession {
                session_id,
                cwd: _,
                expected_mode,
            } => {
                let expected = conversation_mode_from_dto(*expected_mode);
                if self.handle.conversation_mode().await.mode != expected {
                    return Err((
                        ErrorKindDto::Rejected,
                        "session_mode_mismatch: expected mode does not match the runtime".into(),
                    ));
                }
                let uuid = uuid::Uuid::parse_str(session_id).map_err(|error| {
                    (
                        ErrorKindDto::Protocol,
                        format!("resume: malformed session id {session_id:?}: {error}"),
                    )
                })?;
                let store = self.session_store.as_ref().ok_or_else(|| {
                    (
                        ErrorKindDto::Internal,
                        "resume: persisted session store is unavailable".into(),
                    )
                })?;
                let (replayed, source) = self
                    .replay_resume_source(&store.lingxi_home, uuid)
                    .await
                    .map_err(|error| {
                        (
                            ErrorKindDto::Protocol,
                            format!("resume: session {session_id} not resumable: {error}"),
                        )
                    })?;
                let actual = replayed_conversation_mode(&replayed, uuid)
                    .map_err(|error| (ErrorKindDto::Rejected, error))?;
                if actual != expected {
                    return Err((
                        ErrorKindDto::Rejected,
                        "session_mode_mismatch: expected mode does not match the session".into(),
                    ));
                }
                return Ok(Some(PreparedSessionTransition {
                    id: uuid,
                    mode: expected,
                    replayed,
                    source,
                }));
            }
            _ => {}
        }
        Ok(None)
    }

    async fn route_prepared_session_transition(
        &self,
        command: ClientCommand,
        prepared: Option<PreparedSessionTransition>,
        sink: Arc<dyn ClientEventSink>,
    ) {
        if let ClientCommand::ResumeSession {
            session_id,
            expected_mode,
            ..
        } = &command
        {
            let Some(prepared) = prepared else {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Rejected,
                    message: "resume: missing prepared source".into(),
                })
                .await;
                return;
            };
            if uuid::Uuid::parse_str(session_id).ok() != Some(prepared.id)
                || conversation_mode_from_dto(*expected_mode) != prepared.mode
            {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Rejected,
                    message: "resume: prepared source does not match request".into(),
                })
                .await;
                return;
            }
            if let Err(message) = self
                .resume_replayed_session(
                    prepared.id,
                    prepared.replayed,
                    prepared.mode,
                    prepared.source,
                    sink.clone(),
                )
                .await
            {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Rejected,
                    message: message.into(),
                })
                .await;
            }
            return;
        }
        if prepared.is_some() {
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Rejected,
                message: "unexpected prepared session source".into(),
            })
            .await;
            return;
        }
        self.route(command, sink).await;
    }

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
        let expected_mode = request
            .string_at("/expected_mode")
            .and_then(ConversationMode::from_str)
            .ok_or("resume_selected_session requires expected_mode chat or code")?;
        if self.handle.conversation_mode().await.mode != expected_mode {
            return Err("session_mode_mismatch: expected mode differs from the runtime".into());
        }
        if let Some(workspace) = &self.workspace_commands {
            if let Some(proof) = workspace
                .complete_cold_resume(id, &path, expected_mode)
                .await?
            {
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
        let (replayed, source) = replay_pinned_catalog_source(&store.lingxi_home, &path, id)?;
        self.resume_replayed_session(id, replayed, expected_mode, source, sink)
            .await?;
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
    async fn session_metadata(
        &self,
        request: Utf16JsonProjection,
    ) -> Result<Utf16JsonProjection, String> {
        let json = request
            .to_json_string()
            .map_err(|_| "invalid session metadata request")?;
        let request: crate::session_metadata::MetadataMutation =
            serde_json::from_str(&json).map_err(|_| "invalid session metadata request")?;
        let session_id = request.session_id()?;
        let lease = self
            .workspace_commands
            .as_ref()
            .ok_or("session metadata writer authority is unavailable")?
            .current_writer_lease(session_id)
            .await?;
        let writer = self
            .session_store
            .as_ref()
            .and_then(|store| store.transcript_writer.as_ref())
            .ok_or("session metadata writer is unavailable")?
            .clone();
        // The waiter can disappear on disconnect while durable I/O is still
        // settling. The accepted task keeps the source's UUID claim until the
        // writer operation completes, even if this caller is cancelled.
        let result = tokio::spawn(async move {
            let _lease = lease;
            crate::session_metadata::apply_active(&writer, request).await
        })
        .await
        .map_err(|_| "session metadata operation failed")??;
        Utf16JsonProjection::from_serde_value(&result)
            .map_err(|_| "session metadata response is invalid".to_string())
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
        let identity = profile.as_ref().and_then(|profile| {
            let registry = self.media_cloud_audio.as_ref()?;
            let binding = self
                .media_cloud_audio_binding
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone()?;
            let (attachment, session, _) = binding;
            if session != current || attachment.phase() != MediaAttachmentPhase::Active {
                return None;
            }
            let current_attachment = self
                .media_cloud_attachment_slot
                .as_ref()?
                .read()
                .unwrap_or_else(|error| error.into_inner())
                .clone()?;
            if current_attachment.id() != attachment.id() {
                return None;
            }
            let owner = MediaOwner::Session {
                session_id: current.clone(),
            }
            .in_attachment(attachment.id().clone());
            registry
                .session_identity_for_owner(&owner)
                .filter(|(registered_profile, _)| registered_profile == profile)
        });
        let (profile, scope) = identity
            .and_then(|(profile, scope)| {
                audio_context_profile(profile, &scope).map(|profile| (profile, scope))
            })
            .map(|(profile, scope)| (Some(profile), Some(scope)))
            .unwrap_or((None, None));
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
                lingxi_core::host::provider_accounts::UNBOUND_ACCOUNT_SCOPE,
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
            ClientCommand::SideChatRestore { side_id, checkpoint_json, events_json } => {
                let result = match self.side_chat_manager(sink.clone()).await {
                    Ok(manager) => manager.restore(&side_id, &checkpoint_json, events_json).await,
                    Err(error) => Err(error),
                };
                if let Err(message) = result {
                    sink.emit(ClientEvent::SideChatError { request_id: None, side_id: Some(side_id), message }).await;
                }
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
            ClientCommand::CronRunStarted {
                run_id,
                session_id,
                transcript_path,
                session_mode,
                session_scope_key,
            } => {
                if !self.handle.workspace_trusted().await {
                    return;
                }
                if let Some(firer) = &self.cron_firer {
                    firer
                        .started(
                            &run_id,
                            &session_id,
                            &transcript_path,
                            &session_mode,
                            session_scope_key.as_deref(),
                        )
                        .await;
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
                let owner_id = lingxi_core::types::SessionId::parse_prefixed(&status.session_id)
                    .map(|id| id.as_uuid().to_string())
                    .unwrap_or_else(|| {
                        status
                            .session_id
                            .strip_prefix("sess:")
                            .unwrap_or(&status.session_id)
                            .to_owned()
                    });
                if request.action == "create" {
                    let owner = lingxi_core::types::SessionId::parse_prefixed(&owner_id);
                    let mode = self.handle.conversation_mode().await;
                    let title = request
                        .automation
                        .as_ref()
                        .and_then(|automation| automation.name.as_deref())
                        .filter(|name| !name.trim().is_empty())
                        .unwrap_or("Scheduled task");
                    let anchor = match (owner, self.session_store.as_ref()) {
                        (Some(owner), Some(store))
                            if lingxi_core::types::SessionId::parse_prefixed(&mode.session_id)
                                == Some(owner) =>
                        {
                            store
                                .ensure_scheduled_chat(&owner_id, mode.mode, &status.cwd, title)
                                .await
                        }
                        (_, None) => Err("Scheduled task chat storage is unavailable".into()),
                        _ => Err("Scheduled task chat changed before it could be retained".into()),
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
                    let current = self.handle.get_status_snapshot().await;
                    let current_mode = self.handle.conversation_mode().await;
                    if lingxi_core::types::SessionId::parse_prefixed(&current.session_id) != owner
                        || lingxi_core::types::SessionId::parse_prefixed(&current_mode.session_id)
                            != owner
                        || current_mode.mode != mode.mode
                        || current.cwd != status.cwd
                    {
                        sink.emit(ClientEvent::CronResult {
                            request_id,
                            jobs: Vec::new(),
                            error: Some("Scheduled task chat changed before creation".into()),
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
                    &owner_id,
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
            ClientCommand::ProviderAccounts {
                operation_id,
                action,
                request_json,
                credential,
            } => {
                let result = async {
                    let store = self
                        .provider_account_store
                        .as_ref()
                        .ok_or("account store unavailable")?;
                    if action == "inject_credentials" {
                        let mut current = self.provider_account_update_lock.lock().await;
                        if !self.provider_credentials_ephemeral {
                            return Err("private account injection is unavailable".to_string());
                        }
                        let snapshot: lingxi_core::host::provider_accounts::AccountSnapshot =
                            serde_json::from_str(&request_json)
                                .map_err(|_| "invalid account snapshot")?;
                        let mut values: std::collections::BTreeMap<
                            String,
                            secret::AccountCredential,
                        > = serde_json::from_str(
                            credential
                                .as_ref()
                                .ok_or("account credentials required")?
                                .expose_secret(),
                        )
                        .map_err(|_| "invalid account credential envelope")?;
                        crate::provider_accounts::validate_injection(Some(&snapshot), &values)?;
                        if snapshot.store_scope != store.store_scope() {
                            return Err("account store scope mismatch".to_string());
                        }
                        crate::provider_accounts::validate_current_snapshot(
                            store.data_dir(),
                            &snapshot,
                        )?;
                        crate::provider_accounts::validate_credential_revisions(
                            &store.credentials(),
                            &snapshot,
                            &values,
                        )
                        .await?;
                        if let Some(previous) = current.as_ref() {
                            for account in &previous.accounts {
                                if !snapshot
                                    .accounts
                                    .iter()
                                    .any(|next| next.credential_ref == account.credential_ref)
                                {
                                    store
                                        .credentials()
                                        .set_account_credential_ephemeral(
                                            &account.credential_ref,
                                            None,
                                        )
                                        .await
                                        .map_err(|_| "private account credential unavailable")?;
                                }
                            }
                        }
                        for account in &snapshot.accounts {
                            store
                                .credentials()
                                .set_account_credential_ephemeral(
                                    &account.credential_ref,
                                    values.remove(&account.account_id),
                                )
                                .await
                                .map_err(|_| "private account credential unavailable")?;
                        }
                        self.catalog_registry
                            .refresh_provider_accounts(snapshot.clone())?;
                        *current = Some(snapshot.clone());
                        return serde_json::to_string(&snapshot)
                            .map_err(|_| "account snapshot encoding failed".to_string());
                    }
                    let request: lingxi_core::host::provider_accounts::ProviderAccountCommand =
                        serde_json::from_str(&request_json)
                            .map_err(|_| "invalid account command")?;
                    let snapshot = if self.provider_credentials_ephemeral {
                        if credential.is_some() {
                            return Err("Broker owns account credentials".to_string());
                        }
                        store.execute_metadata(request).await?
                    } else {
                        store
                            .execute(request, credential.map(|s| s.expose_secret().to_string()))
                            .await?
                    };
                    self.catalog_registry
                        .refresh_provider_accounts(snapshot.clone())?;
                    serde_json::to_string(&snapshot)
                        .map_err(|_| "account snapshot encoding failed".to_string())
                }
                .await;
                let (snapshot_json, error) = match result {
                    Ok(snapshot) => (snapshot, None),
                    Err(error) => (String::new(), Some(error)),
                };
                sink.emit(ClientEvent::ProviderAccountsUpdated {
                    operation_id,
                    snapshot_json,
                    error,
                })
                .await;
            }
            // ── Permission mode ────────────────────────────────────────────
            ClientCommand::GetContextManifest => {
                let manifest = self.handle.context_manifest().await;
                match serde_json::to_string(&manifest) {
                    Ok(manifest_json) => {
                        sink.emit(ClientEvent::ContextManifest { manifest_json })
                            .await;
                    }
                    Err(error) => {
                        sink.emit(ClientEvent::Error {
                            kind: ErrorKindDto::Internal,
                            message: format!("context manifest serialization failed: {error}")
                                .into(),
                        })
                        .await
                    }
                }
            }
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
            ClientCommand::ListSessions { limit, mode } => {
                let limit = limit.map_or(DEFAULT_SESSION_LIST_LIMIT, |value| value as usize);
                self.emit_session_list(limit, mode, &*sink).await;
            }
            ClientCommand::PrepareSessionHandoff {
                request_id,
                target_mode,
            } => {
                let orchestrator = self
                    .workspace_commands
                    .as_ref()
                    .ok_or_else(|| "handoff runtime is unavailable".to_string())
                    .and_then(|workspace| workspace.orchestrator());
                match orchestrator {
                    Ok(orchestrator) => {
                        let event_sink = sink.clone();
                        let result = self
                            .handoff_manager
                            .prepare(
                                orchestrator,
                                request_id.clone(),
                                target_mode,
                                move |event| {
                                    let event_sink = event_sink.clone();
                                    async move { event_sink.emit(event).await }
                                },
                            )
                            .await;
                        if let Err(message) = result {
                            sink.emit(ClientEvent::SessionHandoffFailed {
                                request_id,
                                message,
                            })
                            .await;
                        }
                    }
                    Err(message) => {
                        sink.emit(ClientEvent::SessionHandoffFailed {
                            request_id,
                            message,
                        })
                        .await
                    }
                }
            }
            ClientCommand::CancelSessionHandoff { request_id } => {
                self.handoff_manager.cancel(&request_id);
            }
            ClientCommand::RetrySessionHandoff { request_id } => {
                let orchestrator = self
                    .workspace_commands
                    .as_ref()
                    .ok_or_else(|| "handoff runtime is unavailable".to_string())
                    .and_then(|workspace| workspace.orchestrator());
                match orchestrator {
                    Ok(orchestrator) => {
                        let event_sink = sink.clone();
                        let result = self
                            .handoff_manager
                            .retry(orchestrator, request_id.clone(), move |event| {
                                let event_sink = event_sink.clone();
                                async move { event_sink.emit(event).await }
                            })
                            .await;
                        if let Err(message) = result {
                            sink.emit(ClientEvent::SessionHandoffFailed {
                                request_id,
                                message,
                            })
                            .await;
                        }
                    }
                    Err(message) => {
                        sink.emit(ClientEvent::SessionHandoffFailed {
                            request_id,
                            message,
                        })
                        .await
                    }
                }
            }
            ClientCommand::ForkSession {
                session_id,
                target_mode,
            } => {
                let result = match uuid::Uuid::parse_str(
                    session_id.strip_prefix("sess:").unwrap_or(&session_id),
                ) {
                    Ok(id) => self
                        .fork_session(id, conversation_mode_from_dto(target_mode))
                        .await
                        .map(|target| (id, target)),
                    Err(error) => Err(format!("fork: malformed session id: {error}")),
                };
                match result {
                    Ok((source, target)) => {
                        sink.emit(ClientEvent::SessionForked {
                            source_session_id: source.to_string(),
                            session_id: target.to_string(),
                            mode: target_mode,
                        })
                        .await
                    }
                    Err(message) => {
                        sink.emit(ClientEvent::Error {
                            kind: ErrorKindDto::Rejected,
                            message: message.into(),
                        })
                        .await
                    }
                }
            }
            ClientCommand::NewSession {
                cwd: _,
                model,
                mode,
            } => {
                if self.is_turn_active() {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "cannot start a new session while a turn is in flight".into(),
                    })
                    .await;
                    return;
                }
                if self.handle.conversation_mode().await.mode != conversation_mode_from_dto(mode) {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Rejected,
                        message: "session_mode_mismatch: create this mode in its own runtime"
                            .into(),
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
                let mode = self.handle.conversation_mode().await;
                sink.emit(ClientEvent::SessionStarted {
                    session_id: mode.session_id.clone(),
                    mode: lower_conversation_mode(mode.mode),
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
            ClientCommand::ResumeSession {
                session_id,
                cwd: _,
                expected_mode,
            } => {
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
                let (replayed, source) = match self
                    .replay_resume_source(&store.lingxi_home, uuid)
                    .await
                {
                    Ok(result) => result,
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
                    .resume_replayed_session(
                        uuid,
                        replayed,
                        conversation_mode_from_dto(expected_mode),
                        source,
                        sink.clone(),
                    )
                    .await
                {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Rejected,
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

#[cfg(test)]
#[path = "router/tests/scheduled_chat_anchor_tests.rs"]
mod scheduled_chat_anchor_tests;

#[cfg(test)]
#[path = "router/tests/runtime_snapshot_tests.rs"]
mod runtime_snapshot_tests;

#[cfg(test)]
#[path = "router/tests/fork_source_tests.rs"]
mod fork_source_tests;

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
