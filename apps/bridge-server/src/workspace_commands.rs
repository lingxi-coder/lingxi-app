//! User-owned workspace commands. These handlers are registered only on the
//! host slash surface; no model tool can call the relocation primitive.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use command_api::model::{BuiltinCommandHandler, CommandResult};
use command_api::parser::ParsedSlashCommand;
use command_api::workspace_directory::{self, AddDirValidation};
use harness_runtime::desktop::DesktopRuntime;
use lingxi_core::host::OrchestratorHandle;
use lingxi_core::types::utf16_json::Utf16JsonProjection;

use crate::settings_bridge::SettingsContext;

fn response(value: serde_json::Value) -> Utf16JsonProjection {
    Utf16JsonProjection::from_serde_value(&value).expect("workspace control response is JSON")
}

fn rejected(reason: &str, message: &str) -> Utf16JsonProjection {
    response(serde_json::json!({"status":"rejected","reason":reason,"message":message}))
}

pub(crate) fn busy_response() -> Utf16JsonProjection {
    rejected("busy", "A turn is in progress \u{2014} the working directory can only change while the session is idle. Wait for the turn to finish (or interrupt it), then retry.")
}

pub(crate) struct WorkspaceCommands {
    host: Arc<RuntimeWorkspaceHost>,
    facts: Arc<WorkspaceSessionFacts>,
    cold_resume: std::sync::Mutex<Option<ColdBootResume>>,
}

/// Private, in-memory continuity across the same UUID's directory remount.
/// Never write this carrier to transcript or shared user defaults.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeControls {
    session_id: uuid::Uuid,
    permission_mode: String,
    reasoning: lingxi_core::host::ReasoningSelection,
    fast_mode: bool,
    proactivity: orchestrator::NativeProactivityRemountState,
}

/// The source captured before Runtime hydration and SessionStart hooks. It is
/// consumed once, after the connected host can answer deferred tool approvals.
pub(crate) struct ColdBootResume {
    session_id: uuid::Uuid,
    transcript_path: PathBuf,
    replayed: Option<orchestrator::ReplayedSession>,
}

pub(crate) struct ColdResumeProof {
    pub(crate) messages: Vec<client::protocol::message::MessageDto>,
    pub(crate) current_usage: Option<lingxi_core::host::CurrentUsageSnapshot>,
}

impl ColdBootResume {
    pub(crate) fn capture(
        cfg: &harness_runtime::desktop::DesktopConfig,
    ) -> Result<Option<Self>, String> {
        if cfg.defer_session_start {
            return Ok(None);
        }
        let (Some(snapshot), Some(path), Some(id)) = (
            cfg.session_resume_snapshot.as_ref(),
            cfg.session_transcript_path.as_ref(),
            cfg.session_id_override.as_deref(),
        ) else {
            return Ok(None);
        };
        let session_id = uuid::Uuid::parse_str(id).map_err(|error| error.to_string())?;
        if uuid::Uuid::parse_str(&snapshot.source_session_id).ok() != Some(session_id) {
            return Err("cold resume snapshot does not match the active session".into());
        }
        let loaded = session::jsonl::route_lines(&snapshot.content);
        let (messages, _) =
            session::jsonl::build_conversation_chain(&loaded, &session_id.to_string());
        // Explicit mobile draft anchors are allowed by the cold loader and
        // have no display baseline or deferred tool calls to replay.
        let replayed = if messages.is_empty()
            && loaded
                .mobile_empty_sessions
                .contains(&session_id.to_string())
        {
            None
        } else {
            Some(
                orchestrator::resume::replay_captured_session(
                    path.clone(),
                    snapshot.content.to_string(),
                    loaded,
                    session_id,
                )
                .map_err(|error| format!("cold resume proof is unavailable: {error}"))?,
            )
        };
        Ok(Some(Self {
            session_id,
            transcript_path: path.clone(),
            replayed,
        }))
    }
}

fn merge_live_display_history(
    display: &mut Vec<lingxi_core::types::ConversationMessage>,
    live: Vec<lingxi_core::types::ConversationMessage>,
) {
    let mut seen = display
        .iter()
        .map(lingxi_core::types::ConversationMessage::id)
        .collect::<std::collections::HashSet<_>>();
    for message in live {
        // Captured display rows retain the complete pre-compact form. Model
        // history may have normalized that same ID for provider context.
        if seen.insert(message.id()) {
            display.push(message);
        }
    }
}

fn merge_live_resume_projection(
    replayed: &mut orchestrator::ReplayedSession,
    live: &[lingxi_core::types::ConversationMessage],
    envelopes: &std::collections::HashMap<
        lingxi_core::types::MessageId,
        lingxi_core::host::TaskDeliveryHistoryEnvelope,
    >,
) {
    use lingxi_core::types::{ContentBlock, ConversationMessage};
    let captured_ids = replayed
        .display_history
        .iter()
        .map(ConversationMessage::id)
        .collect::<std::collections::HashSet<_>>();
    let tool_names = replayed
        .display_history
        .iter()
        .chain(live)
        .filter_map(|message| match message {
            ConversationMessage::Assistant { content, .. }
            | ConversationMessage::User { content, .. } => Some(content),
            _ => None,
        })
        .flatten()
        .filter_map(|block| match block {
            ContentBlock::ToolUse { id, name, .. } => Some((id.to_string(), name.as_str())),
            _ => None,
        })
        .collect::<std::collections::HashMap<_, _>>();
    for message in live {
        if captured_ids.contains(&message.id()) {
            continue;
        }
        let ConversationMessage::User { content, .. } = message else {
            continue;
        };
        let mut result_ids = content.iter().filter_map(|block| match block {
            ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.to_string()),
            _ => None,
        });
        let (Some(id), None) = (result_ids.next(), result_ids.next()) else {
            continue;
        };
        if !tool_names
            .get(&id)
            .is_some_and(|name| orchestrator::CLIENT_STATE_TOOLS.contains(name))
        {
            continue;
        }
        if let Some(raw) = envelopes
            .get(&message.id())
            .and_then(|envelope| envelope.tool_use_result.as_ref())
        {
            replayed.client_state_tool_results.insert(id, raw.clone());
        }
    }
    merge_live_display_history(&mut replayed.display_history, live.to_vec());
}

#[derive(Default)]
pub(crate) struct WorkspaceSessionFacts {
    request_dequeued: AtomicBool,
    resumed: AtomicBool,
}

impl WorkspaceSessionFacts {
    pub(crate) fn record_request_dequeued(&self) {
        self.request_dequeued.store(true, Ordering::SeqCst);
    }
    pub(crate) fn reset(&self, resumed: bool) {
        self.request_dequeued.store(false, Ordering::SeqCst);
        self.resumed.store(resumed, Ordering::SeqCst);
    }
}

impl WorkspaceCommands {
    pub(crate) fn bind_cold_resume(&self, captured: Option<ColdBootResume>) {
        *self
            .cold_resume
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = captured;
    }

    pub(crate) fn discard_cold_resume(&self) {
        self.cold_resume
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
    }

    pub(crate) async fn merge_live_resume_display(
        &self,
        replayed: &mut orchestrator::ReplayedSession,
    ) -> Result<(), String> {
        let runtime = self.host.runtime()?;
        let session = runtime.orchestrator.session();
        let session = session.lock().await;
        merge_live_resume_projection(
            replayed,
            &session.history,
            &session.native_request_history.task_delivery_envelopes,
        );
        Ok(())
    }

    pub(crate) async fn complete_cold_resume(
        &self,
        session_id: uuid::Uuid,
        path: &Path,
    ) -> Result<Option<ColdResumeProof>, String> {
        // Any selected resume consumes the startup opportunity. A later
        // same-path request must be an ordinary owned resume, not a permanent
        // UUID shortcut. Release the mutex before awaiting tool approvals.
        let captured = self
            .cold_resume
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        let Some(mut captured) = captured.filter(|captured| {
            captured.session_id == session_id && captured.transcript_path == path
        }) else {
            return Ok(None);
        };
        let runtime = self.host.runtime()?;
        if runtime.orchestrator.current_session_id().await.as_uuid() != session_id
            || runtime
                .orchestrator
                .session_transcript_writer()
                .map(|writer| writer.active_path())
                .as_deref()
                != Some(captured.transcript_path.as_path())
        {
            return Ok(None);
        }
        let deferred_tools = captured
            .replayed
            .as_mut()
            .map(|replayed| std::mem::take(&mut replayed.runtime_metadata.deferred_tools))
            .unwrap_or_default();
        runtime
            .orchestrator
            .complete_cold_resume_deferred_tools(
                lingxi_core::types::SessionId::from_uuid(session_id),
                captured.transcript_path,
                deferred_tools,
            )
            .await
            .map_err(|error| format!("cold resume completion failed: {error}"))?;
        let messages = if let Some(mut replayed) = captured.replayed {
            self.merge_live_resume_display(&mut replayed).await?;
            client::adapter::lowering::lower_replayed_transcript(&replayed)
        } else {
            let live = runtime.orchestrator.snapshot_history().await;
            client::adapter::lowering::lower_transcript(&live)
        };
        Ok(Some(ColdResumeProof {
            messages,
            current_usage: runtime.orchestrator.snapshot_cost().await.current_usage,
        }))
    }

    pub(crate) fn session_facts(&self) -> Arc<WorkspaceSessionFacts> {
        self.facts.clone()
    }
    pub(crate) async fn add_working_directory(
        &self,
        request: Utf16JsonProjection,
    ) -> Result<(Utf16JsonProjection, Utf16JsonProjection), Utf16JsonProjection> {
        let Some(path) = request
            .string_at("/path")
            .filter(|path| !path.trim().is_empty())
        else {
            return Err("Usage: /add-dir <path>".into());
        };
        let handler = WorkspaceCommandHandler {
            action: WorkspaceAction::AddDirectory,
            host: self.host.clone(),
            home: dirs::home_dir(),
        };
        let parsed = command_api::parse_slash_command(&format!("/add-dir {path}").into())
            .expect("a nonempty add-dir command parses");
        match handler.handle(&parsed).await {
            CommandResult::Done { display } => {
                let state = self.state().await.map_err(Utf16JsonProjection::from)?;
                Ok((state, display.unwrap_or_else(|| "".into())))
            }
            CommandResult::Failed { source } => Err(source.message_projection()),
            _ => Err("The directory command did not complete.".into()),
        }
    }
    pub(crate) async fn state(&self) -> Result<Utf16JsonProjection, String> {
        let runtime = self.host.runtime()?;
        let controls = runtime
            .orchestrator
            .conversation_controls()
            .await
            .ok_or("conversation controls unavailable")?;
        let runtime_controls = serde_json::to_string(&RuntimeControls {
            session_id: runtime.orchestrator.current_session_id().await.as_uuid(),
            permission_mode: runtime
                .orchestrator
                .permission_mode()
                .unwrap_or_else(|| "default".into()),
            reasoning: controls.requested_reasoning_selection,
            fast_mode: runtime.orchestrator.fast_mode().await,
            proactivity: runtime.orchestrator.native_proactivity_remount_state(),
        })
        .map_err(|error| error.to_string())?;
        let has_conversation =
            runtime
                .orchestrator
                .snapshot_history()
                .await
                .iter()
                .any(|message| {
                    matches!(
                        message,
                        lingxi_core::types::ConversationMessage::User { .. }
                            | lingxi_core::types::ConversationMessage::Assistant { .. }
                    )
                });
        let initial_project_root = self
            .host
            .initial_project_root
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let mut additional = self.host.initial_add_dirs.clone();
        for path in runtime.session_cwd.explicit_trusted_dirs() {
            if !additional.contains(&path) {
                additional.push(path);
            }
        }
        Ok(response(serde_json::json!({
            "cwd":workspace_directory::logical_directory_text(&runtime.session_cwd.cwd()),
            "working_directory":runtime.orchestrator.current_cwd().to_string_lossy(),
            "additional_directories":additional,
            "initial_project_root":initial_project_root,
            "transcript_path":runtime.orchestrator.session_transcript_writer().map(|writer|writer.active_path()),
            "prompt_directories":runtime.registered_prompt_directories().await,
            "has_conversation":has_conversation,
            "runtime_controls":runtime_controls,
        })))
    }

    pub(crate) async fn restore_runtime_controls(
        &self,
        request: Utf16JsonProjection,
    ) -> Result<Utf16JsonProjection, String> {
        let encoded = request
            .string_at("/snapshot")
            .filter(|text| text.len() <= 16_384)
            .ok_or("invalid runtime controls snapshot")?;
        let controls: RuntimeControls =
            serde_json::from_str(encoded).map_err(|_| "invalid runtime controls snapshot")?;
        let runtime = self.host.runtime()?;
        if runtime.orchestrator.current_session_id().await.as_uuid() != controls.session_id {
            return Err("runtime controls belong to another session".into());
        }
        runtime
            .orchestrator
            .set_permission_mode(&controls.permission_mode)
            .await?;
        runtime
            .orchestrator
            .set_reasoning_selection(controls.reasoning)
            .await
            .map_err(|error| error.to_string())?;
        runtime
            .orchestrator
            .set_fast_mode(controls.fast_mode)
            .await
            .map_err(|error| error.to_string())?;
        runtime
            .orchestrator
            .restore_native_proactivity_remount_state(controls.proactivity);
        lingxi_core::host::live_sessions::set_process_permission_mode(
            &controls.permission_mode,
            controls.permission_mode == "bypassPermissions",
        );
        Ok(response(serde_json::json!({"status":"ok"})))
    }
    pub(crate) async fn set_working_directory(
        &self,
        request: Utf16JsonProjection,
    ) -> Result<Utf16JsonProjection, String> {
        let raw = request
            .string_at("/path")
            .filter(|path| !path.trim().is_empty())
            .ok_or(permission::set_cwd::INVALID_PATH_MESSAGE)?;
        let trust_accepted = request
            .get_property("trust_accepted")
            .and_then(|value| value.as_bool())
            == Some(true);
        let trusted_directory = request.string_at("/trusted_directory");
        if trust_accepted && trusted_directory.is_none() {
            return Err(permission::set_cwd::INVALID_TRUST_ECHO_MESSAGE.into());
        }
        let runtime = self.host.runtime()?;
        let cwd = runtime.session_cwd.cwd();
        let requested =
            workspace_directory::resolve_path(raw.trim(), &cwd, dirs::home_dir().as_deref());
        if permission::is_network_working_directory_against(raw, cwd.to_str())
            || permission::is_network_working_directory_against(
                &requested.to_string_lossy(),
                cwd.to_str(),
            )
        {
            return Ok(rejected("unsafe_path", "The target is a network path or an obfuscated spelling, which cannot be set as the working directory from a remote host. The path is deliberately not echoed back."));
        }
        let canonical = std::fs::canonicalize(&requested).unwrap_or_else(|_| requested.clone());
        let display = canonical.to_string_lossy().into_owned();
        if permission::set_cwd::has_unprintable(&display) {
            return Ok(rejected(
                "unsafe_path",
                permission::set_cwd::UNSAFE_PATH_MESSAGE,
            ));
        }
        if permission::is_network_working_directory_against(&display, cwd.to_str()) {
            return Ok(rejected("unsafe_path", "The target resolved to a network path or an obfuscated spelling, which cannot be set as the working directory from a remote host. The path is deliberately not echoed back."));
        }
        match std::fs::metadata(&requested) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => {
                return Ok(rejected(
                    "not_a_directory",
                    &format!("{} is not a directory.", requested.display()),
                ))
            }
            Err(_) => {
                return Ok(rejected(
                    "not_found",
                    &format!("Couldn't find a directory at {}.", requested.display()),
                ))
            }
        }
        if same_working_directory(&canonical, &cwd, &runtime.orchestrator.current_cwd()) {
            return Ok(response(
                serde_json::json!({"status":"ok","cwd":display,"changed":false,"transcript_relocated":true}),
            ));
        }
        if let Some(message) = self
            .host
            .cd_permission_rejection(&requested, &canonical, false)?
        {
            return Ok(rejected("blocked_by_rule", &message));
        }
        if !self.host.target_is_trusted(&canonical) {
            if !trust_accepted || trusted_directory != Some(display.as_str()) {
                let mut fields = serde_json::json!({"status":"needs_trust","directory":display});
                if let Some(root) =
                    permission::set_cwd::project_root_of(&canonical).filter(|root| {
                        root != &canonical
                            && !permission::set_cwd::has_unprintable(&root.to_string_lossy())
                    })
                {
                    fields["trust_root"] = root.to_string_lossy().into_owned().into();
                }
                return Ok(response(fields));
            }
            if dirs::home_dir()
                .and_then(|home| std::fs::canonicalize(home).ok())
                .as_deref()
                == Some(canonical.as_path())
            {
                migrations::global_config::set_session_trust_accepted(true);
            } else if let Some(path) = migrations::global_config::global_config_path() {
                migrations::global_config::record_trust_accept(&path, &canonical);
            }
            self.host
                .trusted_workspaces
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(canonical.clone());
        }
        let request_built = self.facts.request_dequeued.load(Ordering::SeqCst);
        let has_assistant = runtime
            .orchestrator
            .snapshot_history()
            .await
            .iter()
            .any(|message| {
                matches!(
                    message,
                    lingxi_core::types::ConversationMessage::Assistant { .. }
                )
            });
        let fresh_session =
            !request_built && !self.facts.resumed.load(Ordering::SeqCst) && !has_assistant;
        runtime
            .change_working_directory(canonical)
            .await
            .map_err(|error| error.to_string())?;
        if fresh_session {
            *self
                .host
                .initial_project_root
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = runtime.session_cwd.cwd();
        }
        persist_move_notice(
            &runtime,
            command_api::cd::MoveMessageOptions {
                source: command_api::cd::MoveSource::User,
                request_built,
                fresh_session,
            },
        )
        .await;
        Ok(response(serde_json::json!({
            "status":"ok", "cwd":workspace_directory::logical_directory_text(&runtime.session_cwd.cwd()),
            "changed":true, "transcript_relocated":true,
        })))
    }
}

#[derive(Clone, Copy)]
enum WorkspaceAction {
    ChangeDirectory,
    AddDirectory,
}

#[async_trait::async_trait]
trait WorkspaceCommandHost: Send + Sync {
    fn cwd(&self) -> Result<PathBuf, String>;
    async fn apply(&self, action: WorkspaceAction, target: PathBuf) -> Result<String, String>;
}

struct WorkspaceCommandHandler {
    action: WorkspaceAction,
    host: Arc<dyn WorkspaceCommandHost>,
    home: Option<PathBuf>,
}

fn failure(message: impl Into<String>) -> CommandResult {
    CommandResult::Failed {
        source: lingxi_core::settings::SettingsError::source_failure(
            "workspace_command",
            message.into().into(),
            Utf16JsonProjection::parse("{}").expect("empty diagnostics object"),
            None,
        ),
    }
}

#[async_trait::async_trait]
impl BuiltinCommandHandler for WorkspaceCommandHandler {
    async fn handle(&self, args: &ParsedSlashCommand) -> CommandResult {
        let Some(input) = args.raw_args.string_at("") else {
            return failure("The directory path is not valid Unicode.");
        };
        let input = input.trim();
        if input.is_empty() {
            return failure(format!("Usage: /{} <path>", self.name()));
        }
        let cwd = match self.host.cwd() {
            Ok(cwd) => cwd,
            Err(error) => return failure(error),
        };
        let validated = match self.action {
            WorkspaceAction::ChangeDirectory => {
                workspace_directory::resolve_existing_directory(input, &cwd, self.home.as_deref())
            }
            WorkspaceAction::AddDirectory => {
                workspace_directory::resolve_and_validate(input, &cwd, self.home.as_deref())
            }
        };
        let AddDirValidation::Success { absolute } = validated else {
            let message = match (&self.action, &validated) {
                (WorkspaceAction::ChangeDirectory, AddDirValidation::PathNotFound { absolute }) => {
                    format!("Couldn't find a directory at {absolute}.")
                }
                (
                    WorkspaceAction::ChangeDirectory,
                    AddDirValidation::NotADirectory { parent, .. },
                ) => format!(
                    "{} is not a directory. Did you mean {parent}?",
                    workspace_directory::resolve_path(input, &cwd, self.home.as_deref()).display()
                ),
                _ => workspace_directory::help_message(&validated),
            };
            return failure(message);
        };
        match self.host.apply(self.action, PathBuf::from(absolute)).await {
            Ok(display) => CommandResult::Done {
                display: Some(display.into()),
            },
            Err(error) => failure(error),
        }
    }

    fn name(&self) -> &str {
        match self.action {
            WorkspaceAction::ChangeDirectory => "cd",
            WorkspaceAction::AddDirectory => "add-dir",
        }
    }

    fn description(&self) -> &str {
        command_api::builtin_support::names::core_description(self.name())
    }

    fn argument_hint(&self) -> Option<&str> {
        Some("<path>")
    }
    fn disable_model_invocation(&self) -> bool {
        true
    }
    fn native_type(&self) -> Option<command_api::model::NativeCommandType> {
        Some(command_api::model::NativeCommandType::LocalJsx)
    }
}

struct RuntimeWorkspaceHost {
    runtime: Weak<DesktopRuntime>,
    settings: SettingsContext,
    lingxi_home: PathBuf,
    trusted_workspaces: std::sync::Mutex<Vec<PathBuf>>,
    initial_add_dirs: Vec<PathBuf>,
    initial_project_root: std::sync::Mutex<PathBuf>,
}

impl RuntimeWorkspaceHost {
    fn runtime(&self) -> Result<Arc<DesktopRuntime>, String> {
        self.runtime
            .upgrade()
            .ok_or_else(|| "The workspace runtime is unavailable.".into())
    }

    fn cd_permission_rejection(
        &self,
        requested: &Path,
        canonical: &Path,
        terminal_affordances: bool,
    ) -> Result<Option<String>, String> {
        let snapshot = self
            .settings
            .snapshot()
            .map_err(|error| error.to_string())?;
        if snapshot.managed_diagnostics.is_some()
            || snapshot.files.iter().any(|file| file.parse_error.is_some())
        {
            return Err(
                "The workspace cannot change while managed settings are unavailable.".into(),
            );
        }
        let mut rules = Vec::new();
        let managed_only = self
            .settings
            .managed
            .get_property("allowManagedPermissionRulesOnly")
            .and_then(|value| value.as_bool())
            == Some(true);
        if !managed_only {
            for (name, scope) in [
                ("user", lingxi_core::types::SettingsScope::User),
                ("project", lingxi_core::types::SettingsScope::Project),
                ("local", lingxi_core::types::SettingsScope::Local),
            ] {
                if let Some(layer) = snapshot.layers.get(name) {
                    rules.extend(
                        permission::permission_rules_from_settings_json(
                            &layer.to_json_string().map_err(|error| error.to_string())?,
                            permission::PermissionRuleSource::Settings(scope),
                        )
                        .map_err(|error| error.to_string())?,
                    );
                }
            }
        }
        rules.extend(
            permission::permission_rules_from_settings_json(
                &self
                    .settings
                    .managed
                    .to_json_string()
                    .map_err(|error| error.to_string())?,
                permission::PermissionRuleSource::Settings(
                    lingxi_core::types::SettingsScope::Managed,
                ),
            )
            .map_err(|error| error.to_string())?,
        );
        let roots = permission::FsRoots {
            cwd: self.settings.paths.project_dir.clone(),
            home: dirs::home_dir(),
            lingxi_home: self.lingxi_home.clone(),
        };
        let check = workspace_directory::check_cd_permission(requested, canonical, &rules, &roots);
        Ok(
            (check != command_api::cd_permissions::CdCheck::Allowed).then(|| {
                let message = command_api::cd_permissions::format_cd_rejection(
                    &canonical.to_string_lossy(),
                    &check,
                    terminal_affordances,
                );
                if permission::set_cwd::has_unprintable(&message) {
                    permission::set_cwd::BLOCKED_BY_UNPRINTABLE_RULE_MESSAGE.into()
                } else {
                    message
                }
            }),
        )
    }

    fn target_is_trusted(&self, target: &Path) -> bool {
        self.trusted_workspaces
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .any(|root| target.starts_with(root))
            || migrations::global_config::global_config_path().is_some_and(|path| {
                migrations::global_config::check_has_trust_dialog_accepted(&path, target)
            })
    }
}

#[async_trait::async_trait]
impl WorkspaceCommandHost for RuntimeWorkspaceHost {
    fn cwd(&self) -> Result<PathBuf, String> {
        Ok(self.runtime()?.session_cwd.cwd())
    }

    async fn apply(&self, action: WorkspaceAction, target: PathBuf) -> Result<String, String> {
        let runtime = self.runtime()?;
        match action {
            WorkspaceAction::ChangeDirectory => {
                let canonical =
                    std::fs::canonicalize(&target).map_err(|error| error.to_string())?;
                if same_working_directory(
                    &canonical,
                    &runtime.session_cwd.cwd(),
                    &runtime.orchestrator.current_cwd(),
                ) {
                    return Ok(format!("Already in {}.", canonical.display()));
                }
                if let Some(message) = self.cd_permission_rejection(&target, &canonical, true)? {
                    return Err(message);
                }
                if !self.target_is_trusted(&canonical) {
                    return Err(
                        "The target workspace must be trusted before changing directory.".into(),
                    );
                }
                let previous = runtime.session_cwd.cwd();
                if let Err(error) = runtime.change_working_directory(canonical.clone()).await {
                    tracing::warn!(%error, "bridge-server: raw /cd relocation failed");
                    return Err(format!("Couldn't move to {} \u{2014} the directory may no longer exist, or the session couldn't be moved. Staying in {}.", canonical.display(), previous.display()));
                }
                persist_move_notice(&runtime, command_api::cd::MoveMessageOptions::default()).await;
                command_api::builtins::cd::emit_command();
                Ok(format!("Moved to {}", runtime.session_cwd.cwd().display()))
            }
            WorkspaceAction::AddDirectory => {
                let cwd = runtime.session_cwd.cwd();
                let target_key = access_path(&target);
                let cwd_key = access_path(&cwd);
                if target_key != cwd_key && target_key.starts_with(&cwd_key) {
                    return include_current_child(&runtime, &target, &cwd).await;
                }
                if let Some(message) =
                    already_accessible_message(&target, &cwd, &runtime.session_cwd.trusted_dirs())
                {
                    return Ok(message);
                }
                // Complete fallible catalog loading before committing the new
                // grant, so a malformed extension can be repaired and retried.
                runtime.register_prompt_directory(target.clone()).await?;
                let added = runtime.session_cwd.add_trusted_dir(target.clone());
                let path = target.to_string_lossy().into_owned();
                if added {
                    runtime.mcp_registry.add_root(target.clone());
                    runtime.mcp_registry.notify_roots_list_changed_all().await;
                    runtime
                        .orchestrator
                        .fire_directory_added(&path, "slash_command")
                        .await;
                }
                if added {
                    if let Some(writer) = runtime.orchestrator.session_transcript_writer() {
                        if let Some(source) = writer.active_path().parent() {
                            if let Err(error) = session::jsonl::record_session_alias(
                                &self.lingxi_home,
                                source,
                                &target,
                            )
                            .await
                            {
                                tracing::warn!(%error, "bridge-server: session alias could not be recorded");
                            }
                        }
                    }
                }
                Ok(if added {
                    format!("Added {path} as a working directory for this session")
                } else {
                    format!("{path} is already added as a working directory.")
                })
            }
        }
    }
}

async fn persist_move_notice(
    runtime: &DesktopRuntime,
    options: command_api::cd::MoveMessageOptions,
) {
    let message = command_api::cd::model_result_message(
        &runtime.session_cwd.cwd().to_string_lossy(),
        options,
    );
    if !message.is_empty() {
        if let Err(error) = lingxi_core::host::OrchestratorHandle::append_meta_user_message(
            runtime.orchestrator.as_ref(),
            &message.into(),
        )
        .await
        {
            tracing::warn!(%error, "bridge-server: directory move notice could not be retained");
        }
    }
}

fn same_working_directory(target: &Path, logical: &Path, working: &Path) -> bool {
    let target = workspace_directory::logical_directory_text(target);
    target == workspace_directory::logical_directory_text(logical)
        && target == workspace_directory::logical_directory_text(working)
}

fn already_accessible_message(target: &Path, cwd: &Path, additional: &[PathBuf]) -> Option<String> {
    let target_key = access_path(target);
    let cwd_key = access_path(cwd);
    if target_key == cwd_key {
        return Some(format!(
            "{} is already the current working directory.",
            target.display()
        ));
    }
    if target_key.starts_with(&cwd_key) {
        return Some(format!(
            "{} is already accessible within the current working directory {}.",
            target.display(),
            cwd.display()
        ));
    }
    for root in additional {
        let root_key = access_path(root);
        if target_key == root_key {
            return Some(format!(
                "{} is already added as a working directory.",
                target.display()
            ));
        }
        if target_key.starts_with(root_key) {
            return Some(format!(
                "{} is already accessible within the additional working directory {}.",
                target.display(),
                root.display()
            ));
        }
    }
    None
}

fn access_path(path: &Path) -> PathBuf {
    let text = workspace_directory::logical_directory_text(path);
    // Native Zu intentionally treats exact /private/var differently from tmp.
    if let Some(tail) = text.strip_prefix("/private/var/") {
        return Path::new("/var").join(tail);
    }
    let path = PathBuf::from(text);
    if let Ok(tail) = path.strip_prefix("/private/tmp") {
        return Path::new("/tmp").join(tail);
    }
    path
}

async fn include_current_child(
    runtime: &DesktopRuntime,
    target: &Path,
    cwd: &Path,
) -> Result<String, String> {
    let prefix = format!(
        "{} is inside the current working directory {}",
        target.display(),
        cwd.display()
    );
    let capabilities = runtime.prompt_directory_capabilities();
    let label = match (capabilities.skills, capabilities.agents) {
        (true, true) => "skills, commands, and agents",
        (true, false) => "skills and commands",
        _ => "agents",
    };
    let disabled = || {
        format!("{prefix}; skills, commands, and agents from additional directories are disabled in this session, so nothing was loaded.")
    };
    if !runtime.prompt_directories_enabled() {
        return Ok(disabled());
    }
    let unresolved = || {
        format!("{} couldn't be resolved to a real location, so its skills, commands, and agents weren't loaded. Check that it is a directory inside the working directory and try again.", target.display())
    };
    let (Ok(canonical_cwd), Ok(canonical)) =
        (std::fs::canonicalize(cwd), std::fs::canonicalize(target))
    else {
        return Ok(unresolved());
    };
    if !canonical.starts_with(canonical_cwd) {
        return Ok(format!("{} leads outside the working directory through a link (it resolves to {}), so its skills, commands, and agents weren't loaded. Run /add-dir {} to grant access to it.", target.display(), canonical.display(), canonical.display()));
    }
    use harness_runtime::desktop::PromptDirectoryRegistration;
    match runtime.register_prompt_directory(canonical).await? {
        PromptDirectoryRegistration::Registered => {
            Ok(format!("{prefix}; loading its {label} for this session."))
        }
        PromptDirectoryRegistration::AlreadyRegistered => Ok(format!(
            "{prefix}; its {label} are already included for this session."
        )),
        PromptDirectoryRegistration::Disabled => Ok(disabled()),
    }
}

pub(crate) async fn register(
    runtime: &Arc<DesktopRuntime>,
    settings: &SettingsContext,
    lingxi_home: PathBuf,
    initial_workspace_trusted: bool,
    initial_add_dirs: Vec<PathBuf>,
    resumed: bool,
) -> Arc<WorkspaceCommands> {
    // The registry is owned by the runtime. Its handlers hold Weak so it can
    // shut down and release transcript writers when BoundServer is dropped.
    let host = Arc::new(RuntimeWorkspaceHost {
        runtime: Arc::downgrade(runtime),
        settings: SettingsContext {
            plugin_archive_session: settings.plugin_archive_session.clone(),
            fusion_layers: settings.fusion_layers.clone(),
            paths: crate::settings_bridge::SettingsPaths {
                lingxi_home: settings.paths.lingxi_home.clone(),
                project_dir: settings.paths.project_dir.clone(),
            },
            active: settings.active.clone(),
            managed: settings.managed.clone(),
            managed_diagnostics: settings.managed_diagnostics.clone(),
        },
        lingxi_home,
        trusted_workspaces: std::sync::Mutex::new(if initial_workspace_trusted {
            vec![runtime.session_cwd.cwd()]
        } else {
            Vec::new()
        }),
        initial_add_dirs,
        initial_project_root: std::sync::Mutex::new(runtime.initial_project_root()),
    });
    let mut registry = runtime.shared_command_registry.write().await;
    for action in [
        WorkspaceAction::ChangeDirectory,
        WorkspaceAction::AddDirectory,
    ] {
        registry.register_builtin_handler(Arc::new(WorkspaceCommandHandler {
            action,
            host: host.clone(),
            home: dirs::home_dir(),
        }));
    }
    drop(registry);
    let facts = Arc::new(WorkspaceSessionFacts::default());
    facts.reset(resumed);
    Arc::new(WorkspaceCommands {
        host,
        facts,
        cold_resume: std::sync::Mutex::new(None),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lingxi_core::host::{SlashCommandDispatcher, SlashDispatchResult};
    use std::sync::Mutex;

    struct Host {
        cwd: PathBuf,
        paths: Mutex<Vec<PathBuf>>,
        reject: bool,
    }

    #[async_trait::async_trait]
    impl WorkspaceCommandHost for Host {
        fn cwd(&self) -> Result<PathBuf, String> {
            Ok(self.cwd.clone())
        }
        async fn apply(&self, action: WorkspaceAction, target: PathBuf) -> Result<String, String> {
            self.paths.lock().unwrap().push(target.clone());
            if self.reject {
                return Err("the move could not be persisted".into());
            }
            Ok(match action {
                WorkspaceAction::ChangeDirectory => {
                    command_api::cd::result_message(&target.to_string_lossy())
                }
                WorkspaceAction::AddDirectory => format!(
                    "Added {} as a working directory for this session",
                    target.display()
                ),
            })
        }
    }

    async fn dispatcher(host: Arc<Host>) -> command_api::RegistrySlashDispatcher {
        let registry = Arc::new(tokio::sync::RwLock::new(command_api::CommandRegistry::new()));
        {
            let mut commands = registry.write().await;
            for action in [
                WorkspaceAction::ChangeDirectory,
                WorkspaceAction::AddDirectory,
            ] {
                commands.register_builtin_handler(Arc::new(WorkspaceCommandHandler {
                    action,
                    host: host.clone(),
                    home: None,
                }));
            }
        }
        let registry_read = registry.read().await;
        assert!(
            registry_read
                .resolve(&"cd".into())
                .unwrap()
                .disable_model_invocation
        );
        assert!(
            registry_read
                .resolve(&"add-dir".into())
                .unwrap()
                .disable_model_invocation
        );
        drop(registry_read);
        command_api::RegistrySlashDispatcher::new(registry)
    }

    #[tokio::test]
    async fn real_workspace_handlers_keep_spaces_unicode_and_live_cwd() {
        let root = tempfile::tempdir().unwrap();
        let child = root.path().join("目录 with spaces");
        std::fs::create_dir(&child).unwrap();
        let host = Arc::new(Host {
            cwd: root.path().into(),
            paths: Mutex::new(Vec::new()),
            reject: false,
        });
        let dispatcher = dispatcher(host.clone()).await;
        for raw in ["/cd 目录 with spaces", "/add-dir 目录 with spaces"] {
            let result = dispatcher
                .dispatch(raw.into(), tokio_util::sync::CancellationToken::new())
                .await;
            assert!(
                matches!(result, SlashDispatchResult::Handled { .. }),
                "{result:?}"
            );
        }
        assert_eq!(*host.paths.lock().unwrap(), vec![child.clone(), child]);
    }

    #[tokio::test]
    async fn workspace_validation_and_move_errors_are_terminal_failures() {
        let root = tempfile::tempdir().unwrap();
        let host = Arc::new(Host {
            cwd: root.path().into(),
            paths: Mutex::new(Vec::new()),
            reject: true,
        });
        let dispatcher = dispatcher(host.clone()).await;
        for raw in [
            "/cd",
            "/add-dir absent",
            "/add-dir //fileserver/share",
            "/cd .",
        ] {
            let result = dispatcher
                .dispatch(raw.into(), tokio_util::sync::CancellationToken::new())
                .await;
            assert!(
                matches!(result, SlashDispatchResult::Failed { .. }),
                "{raw}: {result:?}"
            );
        }
        assert_eq!(
            host.paths.lock().unwrap().len(),
            1,
            "validation failures never call the runtime"
        );
    }

    #[test]
    fn native_initial_busy_shape_precedes_control_validation() {
        let busy = busy_response();
        assert_eq!(busy.string_at("/status"), Some("rejected"));
        assert_eq!(busy.string_at("/reason"), Some("busy"));
        assert_eq!(busy.string_at("/message"), Some("A turn is in progress \u{2014} the working directory can only change while the session is idle. Wait for the turn to finish (or interrupt it), then retry."));
    }

    #[test]
    fn cold_completion_preserves_full_display_rows_and_appends_live_ids_once() {
        use lingxi_core::types::{ConversationMessage, MessageId};
        let historical_id = MessageId::new();
        let shared_id = MessageId::new();
        let added_id = MessageId::new();
        let mut display = vec![
            ConversationMessage::user(historical_id, "pre-compact history".into()),
            ConversationMessage::user(shared_id, "complete display content".into()),
        ];
        merge_live_display_history(
            &mut display,
            vec![
                ConversationMessage::user(shared_id, "normalized model content".into()),
                ConversationMessage::user(added_id, "SessionStart or deferred result".into()),
                ConversationMessage::user(added_id, "duplicate live projection".into()),
            ],
        );
        assert_eq!(
            display
                .iter()
                .map(ConversationMessage::text_content)
                .collect::<Vec<_>>(),
            [
                "pre-compact history",
                "complete display content",
                "SessionStart or deferred result",
            ]
        );
    }

    #[test]
    fn native_add_dir_already_accessible_bytes_and_private_alias_boundaries() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/workspace_add_dir_2_1_307.json"
        ))
        .unwrap();
        for name in [
            "current_directory_short_circuits_alias",
            "project_settings_exact_grant_short_circuits_alias",
            "additional_directory_child_short_circuits_alias",
            "read_fence_retains_user_settings_grant",
        ] {
            let case = fixture["add_dir_cases"]
                .as_array()
                .unwrap()
                .iter()
                .find(|case| case["name"] == name)
                .unwrap();
            let input = &case["input"];
            let additional = input["additional_directories"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|entry| PathBuf::from(entry["path"].as_str().unwrap()))
                .collect::<Vec<_>>();
            let message = already_accessible_message(
                Path::new(input["path"].as_str().unwrap()),
                Path::new(input["current_dir"].as_str().unwrap()),
                &additional,
            )
            .unwrap();
            assert_eq!(
                message,
                case["expected"]["result"]["value"].as_str().unwrap(),
                "{name}"
            );
        }
        assert_eq!(access_path(Path::new("/private/tmp")), Path::new("/tmp"));
        assert_eq!(
            access_path(Path::new("/private/tmpfile")),
            Path::new("/private/tmpfile")
        );
        assert_eq!(
            access_path(Path::new("/private/var")),
            Path::new("/private/var")
        );
        assert_eq!(
            access_path(Path::new("/private/var/child")),
            Path::new("/var/child")
        );
        assert!(already_accessible_message(
            Path::new("/workspace/AA"),
            Path::new("/workspace/A"),
            &[]
        )
        .is_none());
        assert!(already_accessible_message(
            Path::new("/workspace/a"),
            Path::new("/workspace/A"),
            &[]
        )
        .is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn native_control_realigns_actual_bash_cwd_when_logical_cwd_already_matches() {
        let _serial = crate::driver::LOOP_KA_TEST_SERIAL
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let root = tempfile::tempdir().unwrap();
        let launch = root.path().join("launch A");
        let shell = launch.join("shell B");
        std::fs::create_dir(&launch).unwrap();
        std::fs::create_dir(&shell).unwrap();
        let launch = launch.canonicalize().unwrap();
        let shell = shell.canonicalize().unwrap();
        let config_home = root.path().join("isolated-config");
        let mut cfg = harness_runtime::desktop::DesktopConfig::default();
        cfg.cwd = launch.clone();
        cfg.lingxi_home = config_home.clone();
        cfg.initial_project_root = Some(launch.clone());
        cfg.add_dir = vec![shell.clone()];
        cfg.use_noop_permission_gate = false;
        cfg.isolated_credential_storage = true;
        cfg.host_workspace_trusted = Some(true);
        cfg.default_model_explicit = true;
        cfg.enable_automation_scheduler = false;
        cfg.connect_mcp_in_background = false;
        cfg.memory_provider = None;
        cfg.mcp_paths = Vec::new();
        cfg.setting_source_scope = (false, false);
        let permissions = serde_json::json!({"permissions":{"allow":["Bash"]}});
        cfg.flag_settings = Some(serde_json::from_value(permissions.clone()).unwrap());
        cfg.flag_settings_source = Some(permissions.to_string());
        let bound = Box::pin(crate::boot::assemble(cfg)).await.unwrap();
        let runtime = bound.runtime_for_test();
        let empty = Utf16JsonProjection::parse("{}").unwrap();
        let settings = SettingsContext {
            plugin_archive_session: runtime.plugin_archive_session.clone(),
            fusion_layers: None,
            paths: crate::settings_bridge::SettingsPaths {
                lingxi_home: config_home.clone(),
                project_dir: launch.clone(),
            },
            active: Arc::new(std::sync::RwLock::new(empty.clone())),
            managed: empty,
            managed_diagnostics: None,
        };
        let commands = register(
            runtime,
            &settings,
            config_home.clone(),
            true,
            vec![shell.clone()],
            false,
        )
        .await;

        // Remount continuity restores live controls without persisting global
        // defaults or replaying proactivity initialize/level transitions.
        let before_controls = commands.state().await.unwrap();
        let carrier = before_controls
            .string_at("/runtime_controls")
            .unwrap()
            .to_owned();
        let defaults = config_home.join("settings.json");
        let defaults_before = std::fs::read(&defaults).ok();
        runtime
            .orchestrator
            .set_permission_mode("plan")
            .await
            .unwrap();
        runtime
            .orchestrator
            .set_reasoning_selection(lingxi_core::host::ReasoningSelection::Disabled)
            .await
            .unwrap();
        runtime
            .orchestrator
            .apply_native_proactivity_initialize(Some(false), false);
        let restore = |snapshot: &str| response(serde_json::json!({"snapshot": snapshot}));
        let mut wrong: RuntimeControls = serde_json::from_str(&carrier).unwrap();
        wrong.session_id = uuid::Uuid::new_v4();
        assert!(commands
            .restore_runtime_controls(restore(&serde_json::to_string(&wrong).unwrap()))
            .await
            .unwrap_err()
            .contains("another session"));
        assert_eq!(
            runtime.orchestrator.permission_mode().as_deref(),
            Some("plan")
        );
        commands
            .restore_runtime_controls(restore(&carrier))
            .await
            .unwrap();
        assert_eq!(
            commands
                .state()
                .await
                .unwrap()
                .string_at("/runtime_controls"),
            Some(carrier.as_str())
        );
        assert_eq!(std::fs::read(&defaults).ok(), defaults_before);

        let quote = format!("'{}'", shell.to_string_lossy().replace('\'', "'\\''"));
        // Use the registered Bash tool, whose persistent cwd is shared with
        // the orchestrator. The typed `!` runner has a separate per-call cell.
        let output = Box::pin(runtime.orchestrator.call_tool_from_host(
            lingxi_core::types::ToolUseId::new(),
            "Bash".into(),
            response(serde_json::json!({"command":format!("cd {quote}")})),
            None,
        ))
        .await
        .unwrap()
        .expect("the actual Bash tool is registered");
        let lingxi_core::types::ContentBlock::ToolResult {
            is_error, content, ..
        } = output
        else {
            panic!("Bash must return a tool result")
        };
        assert_ne!(is_error, Some(true), "{content:?}");
        assert_eq!(runtime.session_cwd.cwd(), launch);
        assert_eq!(runtime.orchestrator.current_cwd(), shell);
        let before = commands.state().await.unwrap();
        assert_eq!(before.string_at("/cwd"), launch.to_str());
        assert_eq!(before.string_at("/working_directory"), shell.to_str());
        assert_eq!(before.string_at("/initial_project_root"), launch.to_str());

        // Native dt resolves relative input against the logical session cwd A.
        let request = Utf16JsonProjection::parse(r#"{"path":"."}"#).unwrap();
        let moved = commands
            .set_working_directory(request.clone())
            .await
            .unwrap();
        assert_eq!(moved.string_at("/status"), Some("ok"));
        assert_eq!(moved.at_pointer("/changed").unwrap().as_bool(), Some(true));
        assert_eq!(runtime.session_cwd.cwd(), launch);
        assert_eq!(runtime.orchestrator.current_cwd(), launch);
        let same = commands.set_working_directory(request).await.unwrap();
        assert_eq!(same.at_pointer("/changed").unwrap().as_bool(), Some(false));
        let raw = runtime
            .dispatcher
            .dispatch("/cd .".into(), tokio_util::sync::CancellationToken::new())
            .await;
        let SlashDispatchResult::Handled { display } = raw else {
            panic!("expected a completed raw /cd result")
        };
        assert_eq!(
            display.string_at(""),
            Some(format!("Already in {}.", launch.display()).as_str())
        );
        let fresh_target = launch.join("fresh draft");
        let resumed_target = launch.join("resumed session");
        let used_target = launch.join("dequeued request");
        for path in [&fresh_target, &resumed_target, &used_target] {
            std::fs::create_dir(path).unwrap();
        }
        let move_to = |path: &Path| response(serde_json::json!({"path":path}));
        commands.session_facts().reset(false);
        commands
            .set_working_directory(move_to(&fresh_target))
            .await
            .unwrap();
        assert_eq!(
            commands
                .state()
                .await
                .unwrap()
                .string_at("/initial_project_root"),
            fresh_target.to_str()
        );

        // A resumed user-only transcript is used even before its next dequeue.
        commands.session_facts().reset(true);
        commands
            .set_working_directory(move_to(&resumed_target))
            .await
            .unwrap();
        assert_eq!(
            commands
                .state()
                .await
                .unwrap()
                .string_at("/initial_project_root"),
            fresh_target.to_str()
        );
        commands.session_facts().reset(false);
        commands.session_facts().record_request_dequeued();
        commands
            .set_working_directory(move_to(&used_target))
            .await
            .unwrap();
        assert_eq!(
            commands
                .state()
                .await
                .unwrap()
                .string_at("/initial_project_root"),
            fresh_target.to_str()
        );
        let report = bound.session_lifecycle().shutdown_and_drain().await;
        assert!(report.complete, "{:?}", report.errors);
    }
}
