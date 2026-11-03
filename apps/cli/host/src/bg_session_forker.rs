//! `CliBgSessionForker` — the CLI composition root's concrete
//! [`lingxi_core::host::bg_session_forker::BgSessionForker`] (2.1.212 `/fork` `vAd`).
//!
//! Copies the live conversation into a NEW background session and keeps the
//! interactive session running:
//!
//! 1. Mint a fresh session uuid.
//! 2. SNAPSHOT the passed history into that session's
//!    `<config-home>/projects/<sanitize(cwd)>/<uuid>.jsonl` transcript — the
//!    same on-disk shape the resume loader reads — via the session
//!    [`JsonlWriter`], the SAME writer the live session persists through.
//! 3. Dispatch a detached daemon worker that RESUMES that copy
//!    ([`crate::background_dispatch::dispatch_forked_session`], a
//!    `Launch::Resume` job) so the backgrounded turn carries the conversation.
//! 4. Return the live-session system line (owning the minted short id).
//!
//! Layering: this lives in `apps/cli` (which owns the `--bg`/daemon dispatch
//! machinery); it is injected into the orchestrator via
//! `DesktopConfig.bg_session_forker`, so the leaf `orchestrator` crate never
//! depends on `apps/cli`.

use async_trait::async_trait;
use lingxi_core::host::bg_session_forker::lifecycle::{
    BackgroundLaunchLifecycle, BackgroundLaunchRecovery, BackgroundRecoveryTicket,
};
use lingxi_core::host::bg_session_forker::{BgForkError, BgSessionForker, BgSessionSnapshot};
use lingxi_core::host::FileSystem;
#[cfg(unix)]
use platform_posix::PosixFileSystem as HostFileSystem;
#[cfg(windows)]
use platform_windows::WindowsFileSystem as HostFileSystem;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

/// Concrete `/fork`-to-background forker bound to the resolved config/runtime
/// dirs. Constructed in `init::resolve_desktop_config` and set on
/// `DesktopConfig.bg_session_forker`.
pub struct CliBgSessionForker {
    lifecycle: Arc<BackgroundLaunchLifecycle>,
    task_registry: RwLock<Option<Arc<dyn lingxi_core::host::task_registry::TaskRegistryHandle>>>,
    /// `<config-home>` (`~/.lingxi`) — anchors `projects/<cwd>/<uuid>.jsonl` and
    /// the `jobs/<short>/state.json` writes.
    config_home: PathBuf,
    /// Daemon runtime dir (roster + lock).
    runtime_dir: PathBuf,
    /// Engine version string stamped on every snapshotted transcript line.
    version: String,
    /// Full interactive CLI launch context captured at composition time.
    launch_options: crate::background_launch::BackgroundLaunchOptions,
    live_context: RwLock<
        Option<
            Arc<
                dyn Fn() -> Result<(PathBuf, permission::PermissionMode), BgForkError>
                    + Send
                    + Sync,
            >,
        >,
    >,
}

struct ForkPublication {
    conversation: BgSessionSnapshot,
    system_prompt: Option<Arc<lingxi_core::types::utf16_json::Utf16JsonProjection>>,
    prompt: lingxi_core::types::utf16_json::Utf16JsonProjection,
    model: String,
    handoff: Option<lingxi_core::host::BackgroundingSnapshot>,
    cwd: PathBuf,
    context: crate::background_dispatch::ForkLaunchContext,
    config_home: PathBuf,
    runtime_dir: PathBuf,
    version: String,
    session_id: String,
    recovery: Option<Arc<crate::shell_handoff::SourceHandoffRecovery>>,
    ticket: Option<BackgroundRecoveryTicket>,
}

/// An independent fork has a new instruction authority, even though its
/// copied transcript remains available as history.
fn independent_background_snapshot(
    conversation: &BgSessionSnapshot,
) -> Result<BgSessionSnapshot, BgForkError> {
    use lingxi_core::types::ConversationMessage;

    let copied_epoch = conversation
        .history
        .iter()
        .filter_map(|message| match message {
            ConversationMessage::User {
                origin: Some(origin),
                ..
            } if origin.string_at("/type") == Some("skill_activation") => {
                origin.number_at("/context_epoch")
            }
            _ => None,
        })
        .filter(|epoch| epoch.is_finite() && *epoch >= 0.0 && epoch.fract() == 0.0)
        .map(|epoch| epoch as u64)
        .max()
        .unwrap_or(0);
    let mut snapshot = conversation.clone();
    snapshot.instruction_epoch = conversation
        .instruction_epoch
        .max(copied_epoch)
        .checked_add(1)
        .filter(|epoch| *epoch <= (1_u64 << 53) - 1)
        .ok_or_else(|| BgForkError::Snapshot("instruction epoch is exhausted".into()))?;
    Ok(snapshot)
}

fn configure_fork_launch_authority(
    context: &mut crate::background_dispatch::ForkLaunchContext,
    admitted_handoff: bool,
    captured_prompt: Option<Arc<lingxi_core::types::utf16_json::Utf16JsonProjection>>,
) {
    if let Some(options) = &mut context.options {
        options.preserve_admitted_context = admitted_handoff;
    }
    // A computed prompt is scoped to the admitted turn; explicit user launch
    // options remain in `context.options` for either kind of worker.
    context.system_prompt = if admitted_handoff {
        captured_prompt.map(|prompt| prompt.as_ref().clone())
    } else {
        None
    };
}

/// Write the same transcript and host metadata the cold-resume loader reads.
async fn write_background_snapshot(
    writer: &session::jsonl::writer::JsonlWriter,
    conversation: &BgSessionSnapshot,
    session_id: &str,
    cwd: &str,
    version: &str,
    model: &str,
) -> Result<(), BgForkError> {
    // Establish immutable ownership before any conversation history.
    writer
        .initialize_fixed_session_mode(
            session_id,
            conversation.mode,
            conversation.instruction_epoch,
        )
        .await
        .map_err(|error| BgForkError::Snapshot(error.to_string()))?;
    let excluded = conversation
        .model_context_excluded_messages
        .iter()
        .map(|id| id.as_uuid().to_string())
        .collect::<std::collections::HashSet<_>>();
    let lines = orchestrator::bg_snapshot::history_to_jsonl_lines(
        conversation,
        session_id,
        cwd,
        version,
        model,
    );
    for mut line in lines {
        if excluded.contains(&line.uuid) {
            line.extra
                .set_field(
                    "isModelContextExcluded",
                    lingxi_core::types::utf16_json::Utf16JsonProjection::parse("true")
                        .expect("true is valid JSON"),
                )
                .expect("transcript metadata is an object");
        }
        writer
            .append(&line)
            .await
            .map_err(|e| BgForkError::Snapshot(e.to_string()))?;
    }
    Ok(())
}

/// Create the snapshot destination once, retain the identity of the created
/// file, and write through the same durable authority used by its first mount.
fn create_snapshot_writer(
    home: &std::path::Path,
    cwd: &std::path::Path,
    session_id: &str,
    profile: lingxi_core::host::session_context::FixedSessionProfile,
) -> Result<
    (
        session::jsonl::writer::JsonlWriter,
        session::jsonl::writer::TranscriptFileAuthority,
        lingxi_core::host::live_sessions::SessionIdClaim,
    ),
    BgForkError,
> {
    use lingxi_core::host::rooted_fs::{
        ensure_directory_identity_pinned, open_create_new_file_pinned, opened_file_identity,
        root_identity,
    };
    let claim = lingxi_core::host::live_sessions::LiveSessionDir::at_live(home.join("sessions"))
        .claim_session_id(session_id, std::process::id())
        .map_err(|error| BgForkError::Snapshot(error.to_string()))?;
    let root =
        std::fs::canonicalize(home).map_err(|error| BgForkError::Snapshot(error.to_string()))?;
    let identity =
        root_identity(&root).map_err(|error| BgForkError::Snapshot(error.to_string()))?;
    let path = session::jsonl::path::session_path(&root, &cwd.to_string_lossy(), session_id);
    let relative = path
        .strip_prefix(&root)
        .map_err(|error| BgForkError::Snapshot(error.to_string()))?;
    for dir in [
        std::path::PathBuf::from("projects"),
        relative.parent().unwrap().to_path_buf(),
        std::path::PathBuf::from("session-state"),
        std::path::PathBuf::from("session-state").join(session_id),
    ] {
        ensure_directory_identity_pinned(&root, &dir, &identity)
            .map_err(|error| BgForkError::Snapshot(error.to_string()))?;
    }
    if profile.instruction_epoch > (1_u64 << 53) - 1 {
        return Err(BgForkError::Snapshot("invalid instruction epoch".into()));
    }
    let mut file = open_create_new_file_pinned(&root, relative, Some(&identity))
        .map_err(|error| BgForkError::Snapshot(error.to_string()))?;
    let file_identity =
        opened_file_identity(&file).map_err(|error| BgForkError::Snapshot(error.to_string()))?;
    let authority = session::jsonl::writer::TranscriptFileAuthority::from_capture(
        root.clone(),
        relative.to_path_buf(),
        identity,
        file_identity,
    )
    .map_err(|error| BgForkError::Snapshot(error.to_string()))?;
    let header = format!(
        "{}\n",
        serde_json::json!({
            "type": "fixed-session-mode",
            "sessionId": session_id,
            "sessionMode": profile.mode.as_str(),
            "instructionEpoch": profile.instruction_epoch,
        })
    );
    use std::io::Write;
    file.write_all(header.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|error| BgForkError::Snapshot(error.to_string()))?;
    if authority
        .read_checked()
        .map_err(|error| BgForkError::Snapshot(error.to_string()))?
        != header.as_bytes()
    {
        return Err(BgForkError::Snapshot(
            "snapshot source changed while initializing".into(),
        ));
    }
    let durable = Arc::new(
        session::jsonl::DurableTranscriptWriter::open(root.join("session-state").join(session_id))
            .map_err(|error| BgForkError::Snapshot(error.to_string()))?,
    );
    let fs: Arc<dyn FileSystem> = Arc::new(HostFileSystem::new(cwd.to_path_buf()));
    let writer =
        session::jsonl::writer::JsonlWriter::new(path.clone(), fs).with_durable_lock(durable);
    let id = lingxi_core::types::SessionId::parse_prefixed(session_id)
        .ok_or_else(|| BgForkError::Snapshot("invalid snapshot session ID".into()))?;
    writer
        .activate_session_target_with_authority(
            id,
            path,
            cwd.to_path_buf(),
            Some(authority.clone()),
        )
        .map_err(|error| BgForkError::Snapshot(error.to_string()))?;
    Ok((writer, authority, claim))
}

fn pin_resume_source(
    config_home: &std::path::Path,
    cwd: &std::path::Path,
    session_id: &str,
    context: &mut crate::background_dispatch::ForkLaunchContext,
) -> Result<(), BgForkError> {
    let uuid = uuid::Uuid::parse_str(session_id)
        .map_err(|error| BgForkError::Dispatch(format!("invalid session id: {error}")))?;
    let source = crate::background_dispatch::locate_session_transcript(
        config_home,
        &cwd.display().to_string(),
        session_id,
    )
    .map_err(|error| BgForkError::Dispatch(error.to_string()))?;
    let captured = crate::init::capture_selected_cli_source(
        config_home,
        uuid,
        &source,
        Some(lingxi_core::host::session_context::ConversationMode::Code),
    )
    .map_err(|error| BgForkError::Dispatch(error.to_string()))?;
    context.transcript_path = Some(captured.authority.path().to_string_lossy().into_owned());
    context.published_branch = Some(captured);
    Ok(())
}

impl CliBgSessionForker {
    /// Construct a forker over the resolved config-home + daemon runtime dir.
    #[must_use]
    pub fn new(
        config_home: PathBuf,
        runtime_dir: PathBuf,
        launch_options: crate::background_launch::BackgroundLaunchOptions,
        _resolved_permission_mode: permission::PermissionMode,
    ) -> Self {
        Self {
            lifecycle: Arc::new(BackgroundLaunchLifecycle::default()),
            task_registry: RwLock::new(None),
            config_home,
            runtime_dir,
            version: env!("CARGO_PKG_VERSION").to_string(),
            launch_options,
            live_context: RwLock::new(None),
        }
    }

    /// Bind the same runtime authority used by tools after assembly. A weak
    /// orchestrator reference avoids retaining the runtime through its forker.
    pub(crate) fn bind_runtime(
        &self,
        orchestrator: &Arc<orchestrator::ConversationOrchestrator>,
        cwd: Arc<tool_api::SessionCwd>,
    ) {
        let orchestrator = Arc::downgrade(orchestrator);
        *self.live_context.write().expect("background context lock") = Some(Arc::new(move || {
            let runtime = orchestrator.upgrade().ok_or_else(|| {
                BgForkError::Dispatch("foreground runtime is no longer available".into())
            })?;
            let mode = runtime.permission_mode().ok_or_else(|| {
                BgForkError::Dispatch("foreground permission mode is unavailable".into())
            })?;
            let mode = match mode.as_str() {
                "default" | "manual" | "plan" | "acceptEdits" | "bypassPermissions" | "dontAsk"
                | "auto" => permission::permission_mode_from_cli_string(&mode),
                _ => {
                    return Err(BgForkError::Dispatch(
                        "foreground permission mode is not recognized".into(),
                    ))
                }
            };
            Ok((cwd.cwd(), mode))
        }));
    }

    pub(crate) fn launch_identity(
        &self,
    ) -> Result<(PathBuf, crate::background_dispatch::ForkLaunchContext), BgForkError> {
        self.lifecycle.ensure_open()?;
        self.admitted_launch_identity()
    }

    fn admitted_launch_identity(
        &self,
    ) -> Result<(PathBuf, crate::background_dispatch::ForkLaunchContext), BgForkError> {
        let live = self
            .live_context
            .read()
            .expect("background context lock")
            .clone();
        let (cwd, mode) = match live {
            Some(read) => read()?,
            None => {
                return Err(BgForkError::Dispatch(
                    "foreground runtime authority is not bound".into(),
                ));
            }
        };
        Ok((cwd, self.launch_context(mode)))
    }

    fn launch_context(
        &self,
        mode: permission::PermissionMode,
    ) -> crate::background_dispatch::ForkLaunchContext {
        crate::background_dispatch::ForkLaunchContext {
            options: Some(self.launch_options.clone()),
            resolved_permission_mode: Some(mode),
            ..crate::background_dispatch::ForkLaunchContext::default()
        }
    }

    async fn fork_with_handoff(
        &self,
        conversation: &BgSessionSnapshot,
        system_prompt: Option<Arc<lingxi_core::types::utf16_json::Utf16JsonProjection>>,
        prompt: &lingxi_core::types::utf16_json::Utf16JsonProjection,
        model: &str,
        handoff: Option<&lingxi_core::host::BackgroundingSnapshot>,
    ) -> Result<lingxi_core::types::utf16_json::Utf16JsonProjection, BgForkError> {
        // Admit before I/O and transfer ownership to a worker that survives
        // cancellation of the caller. Shutdown joins that worker before cleanup.
        let claim = self.lifecycle.claim()?;
        let (cwd, context) = self.admitted_launch_identity()?;
        let session_id = uuid::Uuid::new_v4().to_string();
        let registry = self
            .task_registry
            .read()
            .expect("background registry lock")
            .clone();
        let recovery = if handoff.is_some() {
            registry.map(|registry| {
                Arc::new(crate::shell_handoff::SourceHandoffRecovery::new(
                    self.config_home.clone(),
                    session_id.clone(),
                    registry,
                ))
            })
        } else {
            None
        };
        // Registration precedes export: a panic leaves the recovery object in
        // the lifecycle until the registry's export guard or retry settles it.
        let ticket = recovery
            .as_ref()
            .map(|recovery| claim.register_recovery(recovery.clone()));
        let publication = ForkPublication {
            conversation: conversation.clone(),
            system_prompt,
            prompt: prompt.clone(),
            model: model.to_string(),
            handoff: handoff.cloned(),
            cwd,
            context,
            config_home: self.config_home.clone(),
            runtime_dir: self.runtime_dir.clone(),
            version: self.version.clone(),
            session_id,
            recovery,
            ticket,
        };
        claim.run_owned(Self::publish_fork(publication)).await
    }

    async fn publish_fork(
        publication: ForkPublication,
    ) -> Result<lingxi_core::types::utf16_json::Utf16JsonProjection, BgForkError> {
        let ForkPublication {
            conversation,
            system_prompt,
            prompt,
            model,
            handoff,
            cwd: cwd_pb,
            context: mut launch_context,
            config_home,
            runtime_dir,
            version,
            session_id,
            recovery,
            mut ticket,
        } = publication;
        configure_fork_launch_authority(&mut launch_context, handoff.is_some(), system_prompt);
        let cwd = cwd_pb.display().to_string();
        let (writer, _initial_authority, target_claim) = match create_snapshot_writer(
            &config_home,
            &cwd_pb,
            &session_id,
            lingxi_core::host::session_context::FixedSessionProfile {
                mode: conversation.mode,
                instruction_epoch: conversation.instruction_epoch,
            },
        ) {
            Ok(created) => created,
            Err(error) => {
                if let Some(ticket) = ticket.take() {
                    ticket.confirm_resolved();
                }
                return Err(error);
            }
        };
        if let Err(error) =
            write_background_snapshot(&writer, &conversation, &session_id, &cwd, &version, &model)
                .await
        {
            // Export has not started, so no shell authority needs recovery.
            if let Some(ticket) = ticket.take() {
                ticket.confirm_resolved();
            }
            return Err(error);
        }
        // The copied transcript must remain the worker's source even if it is
        // removed before dispatch; never fall back to another project's UUID.
        // An owned compaction may have transferred the authority while the
        // snapshot was written; retain that transaction's current target.
        let authority = match writer.active_transcript_authority() {
            Some(authority) => authority,
            None => {
                if let Some(ticket) = ticket.take() {
                    ticket.confirm_resolved();
                }
                return Err(BgForkError::Snapshot(
                    "snapshot writer lost its source authority".into(),
                ));
            }
        };
        let source = match authority.read_checked().and_then(|bytes| {
            String::from_utf8(bytes)
                .map_err(|error| lingxi_core::host::FsError::Io(error.to_string()))
        }) {
            Ok(content) => crate::init::PublishedBranchSource {
                authority: authority.clone(),
                content,
            },
            Err(error) => {
                if let Some(ticket) = ticket.take() {
                    ticket.confirm_resolved();
                }
                return Err(BgForkError::Snapshot(error.to_string()));
            }
        };
        launch_context.transcript_path = Some(authority.path().to_string_lossy().into_owned());
        launch_context.published_branch = Some(source);
        launch_context.session_profile = Some(crate::background_launch::BackgroundSessionProfile {
            mode: conversation.mode,
            instruction_epoch: conversation.instruction_epoch,
        });
        launch_context.model = Some(model);
        launch_context.handoff = handoff;
        if let Some(recovery) = &recovery {
            let exported = match recovery.export().await {
                Ok(exported) => exported,
                Err(error) => {
                    // The registry's export guard releases the fences on error.
                    if let Some(ticket) = ticket.take() {
                        ticket.confirm_resolved();
                    }
                    return Err(BgForkError::Dispatch(error));
                }
            };
            // No await between export and recording the exact recovery roster.
            recovery.record_export(exported.clone());
            launch_context.shell_handoff = exported;
        }
        let remember_short = |short: &str| {
            if let Some(recovery) = &recovery {
                recovery.record_short(short);
            }
        };
        // The durable receipt bridges the handoff after releasing the source
        // claim so the child can acquire the target lease itself.
        drop(target_claim);
        let short = crate::background_dispatch::dispatch_forked_session_with_context(
            &config_home,
            &runtime_dir,
            &cwd,
            &session_id,
            &prompt,
            &launch_context,
            Some(&remember_short),
        );
        let short = match short {
            Ok(short) => short,
            Err(error) => {
                return Err(Self::recover_failed_publication(
                    BgForkError::Dispatch(error.to_string()),
                    recovery.as_deref(),
                    ticket.take(),
                )
                .await);
            }
        };
        if let Some(recovery) = &recovery {
            if let Err(error) = recovery.finish_source().await {
                return Err(Self::recover_failed_publication(
                    BgForkError::Dispatch(error),
                    Some(recovery.as_ref()),
                    ticket.take(),
                )
                .await);
            }
        }
        if let Some(ticket) = ticket.take() {
            ticket.confirm_resolved();
        }
        Ok(format!("Copied conversation into a new background session ({short}).").into())
    }

    async fn recover_failed_publication(
        original: BgForkError,
        recovery: Option<&crate::shell_handoff::SourceHandoffRecovery>,
        ticket: Option<BackgroundRecoveryTicket>,
    ) -> BgForkError {
        if let Some(recovery) = recovery {
            match recovery.recover().await {
                Ok(()) => {
                    if let Some(ticket) = ticket {
                        ticket.confirm_resolved();
                    }
                }
                Err(error) => {
                    // Dropping the ticket does not clear it; shutdown retries
                    // using the same registry and recorded transaction state.
                    return BgForkError::OwnershipUncertain(format!(
                        "{original}; shell handoff recovery pending: {error}"
                    ));
                }
            }
        }
        original
    }
}

#[async_trait]
impl BgSessionForker for CliBgSessionForker {
    fn lifecycle(&self) -> Arc<BackgroundLaunchLifecycle> {
        self.lifecycle.clone()
    }

    fn set_task_registry(
        &self,
        registry: Arc<dyn lingxi_core::host::task_registry::TaskRegistryHandle>,
    ) {
        *self
            .task_registry
            .write()
            .expect("background registry lock") = Some(registry);
    }

    async fn fork_to_background(
        &self,
        conversation: &BgSessionSnapshot,
        system_prompt: Option<Arc<lingxi_core::types::utf16_json::Utf16JsonProjection>>,
        prompt: &lingxi_core::types::utf16_json::Utf16JsonProjection,
        model: &str,
    ) -> Result<lingxi_core::types::utf16_json::Utf16JsonProjection, BgForkError> {
        if conversation.mode != lingxi_core::host::session_context::ConversationMode::Code {
            return Err(BgForkError::Dispatch(
                "session_mode_mismatch: public CLI forks require a Code session".into(),
            ));
        }
        let independent = independent_background_snapshot(conversation)?;
        self.fork_with_handoff(&independent, system_prompt, prompt, model, None)
            .await
    }

    async fn background_conversation(
        &self,
        conversation: &BgSessionSnapshot,
        system_prompt: Option<Arc<lingxi_core::types::utf16_json::Utf16JsonProjection>>,
        prompt: &lingxi_core::types::utf16_json::Utf16JsonProjection,
        model: &str,
        snapshot: &lingxi_core::host::BackgroundingSnapshot,
    ) -> Result<lingxi_core::types::utf16_json::Utf16JsonProjection, BgForkError> {
        self.fork_with_handoff(conversation, system_prompt, prompt, model, Some(snapshot))
            .await
    }

    async fn resume_to_background(&self, session_id: &str) -> Result<String, BgForkError> {
        // The chosen session ALREADY exists on disk at its standard transcript
        // path (`list_resumable_sessions` enumerated it), so — unlike the fork
        // path — there is NO snapshot to write: dispatch a detached daemon that
        // resumes `session_id` directly via the same `Launch::Resume` machinery.
        let claim = self.lifecycle.claim()?;
        let (cwd_pb, mut launch_context) = self.admitted_launch_identity()?;
        // Pin the selected catalog path before publishing a job. If the file
        // disappears, dispatch must not select the UUID in another project.
        pin_resume_source(&self.config_home, &cwd_pb, session_id, &mut launch_context)?;
        let source = launch_context
            .published_branch
            .as_ref()
            .ok_or_else(|| BgForkError::Dispatch("selected resume source is unavailable".into()))?;
        let target_cwd =
            crate::background_dispatch::transcript_cwd(&source.authority.path(), &source.content)
                .map_err(|error| BgForkError::Dispatch(error.to_string()))?;
        let target = std::path::Path::new(&target_cwd);
        if !target.is_dir() {
            return Err(BgForkError::Dispatch(
                "resume workspace is not an existing directory".into(),
            ));
        }
        if target != cwd_pb.as_path()
            && migrations::global_config::global_config_path().is_some_and(|config| {
                !migrations::global_config::check_has_trust_dialog_accepted(&config, target)
            })
        {
            return Err(BgForkError::Dispatch(
                "resume workspace is not trusted; open it and approve trust first".into(),
            ));
        }
        let config_home = self.config_home.clone();
        let runtime_dir = self.runtime_dir.clone();
        let session_id = session_id.to_string();
        // Same grounding caveat as `fork_to_background`: the 2.1.212 resume-as-bg
        // dispatch has no recoverable oracle line (the TUI special dispatcher
        // renders it), so this is grounded on the minted short id, NOT byte-verified.
        claim
            .run_owned(async move {
                let short = crate::background_dispatch::dispatch_resumed_session_with_context(
                    &config_home,
                    &runtime_dir,
                    &cwd_pb.display().to_string(),
                    &session_id,
                    &launch_context,
                )
                .map_err(|e| BgForkError::Dispatch(e.to_string()))?;
                Ok(format!(
                    "Resumed session into a new background session ({short})."
                ))
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lingxi_core::host::orchestrator::OrchestratorHandle;
    use lingxi_core::host::session_context::{ConversationMode, FixedSessionProfile};
    use lingxi_core::types::{
        utf16_json::Utf16JsonProjection as Source, ConversationMessage, MessageId,
    };

    #[tokio::test]
    async fn snapshot_writer_carries_created_inode_and_never_reuses_existing_target() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let cwd = dir.path().join("project");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let profile = FixedSessionProfile {
            mode: ConversationMode::Code,
            instruction_epoch: 1,
        };
        let (writer, authority, claim) = create_snapshot_writer(&home, &cwd, &id, profile).unwrap();
        writer
            .initialize_fixed_session_mode(&id, ConversationMode::Code, 1)
            .await
            .unwrap();
        let bytes = authority.read_checked().unwrap();
        let path = authority.path();
        assert!(writer.verify_active_transcript_source(&authority).is_ok());
        drop(claim);
        assert!(create_snapshot_writer(&home, &cwd, &id, profile).is_err());
        let saved = path.with_extension("saved");
        std::fs::rename(&path, &saved).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        assert!(authority.read_checked().is_err());
    }

    #[tokio::test]
    async fn closed_foreground_rejects_all_launches_before_publication() {
        let directory = tempfile::tempdir().unwrap();
        let config_home = directory.path().join("uncreated-config");
        let runtime_dir = directory.path().join("uncreated-runtime");
        let forker = Arc::new(CliBgSessionForker::new(
            config_home.clone(),
            runtime_dir.clone(),
            crate::background_launch::BackgroundLaunchOptions::default(),
            permission::PermissionMode::AcceptEdits,
        ));
        let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        *forker.live_context.write().unwrap() = Some(Arc::new({
            let reads = reads.clone();
            let cwd = directory.path().to_path_buf();
            move || {
                reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok((cwd.clone(), permission::PermissionMode::Plan))
            }
        }));
        assert!(forker.launch_identity().is_ok());
        let retained = forker.clone();
        let report = forker.lifecycle.close_and_drain().await;
        assert!(report.unconfirmed_recoveries.is_empty());
        let prompt = "".into();
        let conversation = BgSessionSnapshot::default();
        assert!(retained
            .fork_to_background(&conversation, None, &prompt, "model")
            .await
            .is_err());
        assert!(retained
            .background_conversation(&conversation, None, &prompt, "model", &Default::default())
            .await
            .is_err());
        assert!(retained
            .resume_to_background(&uuid::Uuid::nil().to_string())
            .await
            .is_err());
        assert!(retained.launch_identity().is_err());
        assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(!config_home.exists());
        assert!(!runtime_dir.exists());
    }

    #[tokio::test]
    async fn accepted_claim_keeps_its_live_identity_after_closure() {
        let forker = CliBgSessionForker::new(
            PathBuf::from("/tmp/config"),
            PathBuf::from("/tmp/runtime"),
            crate::background_launch::BackgroundLaunchOptions::default(),
            permission::PermissionMode::Plan,
        );
        *forker.live_context.write().unwrap() = Some(Arc::new(|| {
            Ok((PathBuf::from("/tmp"), permission::PermissionMode::Plan))
        }));
        let claim = forker.lifecycle.claim().unwrap();
        forker.lifecycle.close();
        assert!(forker.launch_identity().is_err());
        assert!(forker.admitted_launch_identity().is_ok());
        drop(claim);
        assert!(forker
            .lifecycle
            .close_and_drain()
            .await
            .unconfirmed_recoveries
            .is_empty());
    }

    #[test]
    fn pinned_resume_path_cannot_fall_back_to_another_project() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("config");
        let cwd = dir.path().join("workspace-a");
        let other = dir.path().join("workspace-b");
        let session_id = uuid::Uuid::new_v4().to_string();
        let source =
            session::jsonl::path::session_path(&home, &cwd.display().to_string(), &session_id);
        let replacement =
            session::jsonl::path::session_path(&home, &other.display().to_string(), &session_id);
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::create_dir_all(replacement.parent().unwrap()).unwrap();
        let transcript = |text: &str| {
            let header = serde_json::json!({
                "type": "fixed-session-mode", "sessionId": session_id,
                "sessionMode": "code", "instructionEpoch": 0,
            });
            let message = serde_json::json!({
                "type": "user", "uuid": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
                "parentUuid": null, "sessionId": session_id,
                "timestamp": "2026-10-21T00:00:00.000Z", "isSidechain": false,
                "message": { "role": "user", "content": text },
            });
            format!("{header}\n{message}\n")
        };
        std::fs::write(&source, transcript("original")).unwrap();
        let mut context = crate::background_dispatch::ForkLaunchContext::default();
        pin_resume_source(&home, &cwd, &session_id, &mut context).unwrap();
        assert_eq!(
            std::path::PathBuf::from(context.transcript_path.as_ref().unwrap()),
            source.canonicalize().unwrap()
        );
        std::fs::rename(&source, source.with_extension("moved")).unwrap();
        let replacement_content = transcript("different source");
        std::fs::write(&replacement, &replacement_content).unwrap();
        assert!(
            crate::background_dispatch::dispatch_resumed_session_with_context(
                &home,
                &dir.path().join("runtime"),
                &cwd.display().to_string(),
                &session_id,
                &context,
            )
            .is_err()
        );
        assert!(
            crate::background_dispatch::dispatch_forked_session_with_context(
                &home,
                &dir.path().join("runtime"),
                &cwd.display().to_string(),
                &session_id,
                &"".into(),
                &context,
                None,
            )
            .is_err()
        );
        assert!(!home.join("jobs").exists());
        assert_eq!(
            std::fs::read_to_string(replacement).unwrap(),
            replacement_content
        );
        assert!(pin_resume_source(&home, &cwd, "../other", &mut context).is_err());
    }

    #[tokio::test]
    async fn closed_foreground_rejects_all_launches_before_filesystem_publication() {
        let directory = tempfile::tempdir().unwrap();
        let config_home = directory.path().join("uncreated-config");
        let runtime_dir = directory.path().join("uncreated-runtime");
        let forker = CliBgSessionForker::new(
            config_home.clone(),
            runtime_dir.clone(),
            crate::background_launch::BackgroundLaunchOptions::default(),
            permission::PermissionMode::AcceptEdits,
        );
        let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        *forker.live_context.write().unwrap() = Some(Arc::new({
            let reads = reads.clone();
            let cwd = directory.path().to_path_buf();
            move || {
                reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok((cwd.clone(), permission::PermissionMode::Plan))
            }
        }));
        assert!(forker.launch_identity().is_ok());
        assert!(forker
            .lifecycle()
            .close_and_drain()
            .await
            .unconfirmed_recoveries
            .is_empty());
        let conversation = BgSessionSnapshot::default();
        let prompt = "".into();
        assert!(forker
            .fork_to_background(&conversation, None, &prompt, "model")
            .await
            .is_err());
        assert!(forker
            .background_conversation(&conversation, None, &prompt, "model", &Default::default())
            .await
            .is_err());
        assert!(forker
            .resume_to_background(&uuid::Uuid::nil().to_string())
            .await
            .is_err());
        assert!(forker.launch_identity().is_err());
        assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(
            !config_home.exists(),
            "a rejected copy must not write a transcript or job"
        );
        assert!(
            !runtime_dir.exists(),
            "a rejected launch must not start daemon discovery"
        );
    }

    #[test]
    fn unbound_background_forker_rejects_startup_authority() {
        let forker = CliBgSessionForker::new(
            PathBuf::from("/tmp/config"),
            PathBuf::from("/tmp/runtime"),
            crate::background_launch::BackgroundLaunchOptions::default(),
            permission::PermissionMode::AcceptEdits,
        );
        assert!(forker.launch_identity().is_err());
    }

    #[test]
    fn live_fork_context_preserves_launch_options_and_resolved_mode() {
        let forker = CliBgSessionForker::new(
            PathBuf::from("/tmp/config"),
            PathBuf::from("/tmp/runtime"),
            crate::background_launch::BackgroundLaunchOptions {
                model: Some("captured-model".to_string()),
                agent: Some("reviewer".to_string()),
                allowed_tools: Some(vec!["Read".to_string()]),
                mcp_config: Some(vec!["mcp.json".to_string()]),
                ..crate::background_launch::BackgroundLaunchOptions::default()
            },
            permission::PermissionMode::Plan,
        );

        let context = forker.launch_context(permission::PermissionMode::Plan);
        let options = context.options.unwrap();
        assert_eq!(options.model.as_deref(), Some("captured-model"));
        assert_eq!(options.agent.as_deref(), Some("reviewer"));
        assert_eq!(
            options.allowed_tools.as_ref().unwrap(),
            &vec!["Read".to_string()]
        );
        assert_eq!(
            options.mcp_config.as_ref().unwrap(),
            &vec!["mcp.json".to_string()]
        );
        assert_eq!(
            context.resolved_permission_mode,
            Some(permission::PermissionMode::Plan)
        );
    }

    #[test]
    fn independent_fork_rebuilds_computed_prompt_but_handoff_retains_it() {
        for admitted in [false, true] {
            let mut context = crate::background_dispatch::ForkLaunchContext {
                options: Some(crate::background_launch::BackgroundLaunchOptions {
                    system_prompt: Some("explicit user prompt".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            configure_fork_launch_authority(
                &mut context,
                admitted,
                Some(Arc::new("captured computed prompt".into())),
            );
            let options = context.options.unwrap();
            assert_eq!(options.preserve_admitted_context, admitted);
            assert_eq!(
                options
                    .system_prompt
                    .as_ref()
                    .and_then(|prompt| prompt.string_at("")),
                Some("explicit user prompt")
            );
            assert_eq!(
                context
                    .system_prompt
                    .as_ref()
                    .and_then(|prompt| prompt.string_at("")),
                admitted.then_some("captured computed prompt")
            );
        }
    }

    #[tokio::test]
    async fn independent_background_fork_retires_copied_skill_authority() {
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().join("workspace");
        std::fs::create_dir_all(&cwd).unwrap();
        let home = root.path().join("config");
        let session_id = uuid::Uuid::new_v4();
        let old_id = MessageId::new();
        let future_id = MessageId::new();
        let user_id = MessageId::new();
        let origin = |epoch| {
            Source::parse(&format!(
            r#"{{"type":"skill_activation","context_epoch":{epoch},"skill_name":"x","skill_path":"/skill"}}"#
        )).unwrap()
        };
        let source = BgSessionSnapshot {
            history: vec![
                ConversationMessage::user_meta(old_id, "same evidence".into())
                    .with_origin(origin(4)),
                ConversationMessage::user_meta(future_id, "later instruction".into())
                    .with_origin(origin(9)),
                ConversationMessage::user(user_id, "same evidence".into()),
            ],
            mode: ConversationMode::Code,
            instruction_epoch: 4,
            ..Default::default()
        };
        let independent = independent_background_snapshot(&source).unwrap();
        assert_eq!(independent.instruction_epoch, 10);
        assert_eq!(source.instruction_epoch, 4);
        let path = session::jsonl::path::session_path(
            &home,
            &cwd.to_string_lossy(),
            &session_id.to_string(),
        );
        let fs: Arc<dyn FileSystem> = Arc::new(HostFileSystem::new(cwd.clone()));
        let writer = session::JsonlWriter::new(path.clone(), fs);
        write_background_snapshot(
            &writer,
            &independent,
            &session_id.to_string(),
            &cwd.to_string_lossy(),
            "test",
            "model",
        )
        .await
        .unwrap();
        let persisted = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            session::JsonlWriter::fixed_session_profile_from_source(
                &persisted,
                &session_id.to_string()
            )
            .unwrap(),
            FixedSessionProfile {
                mode: ConversationMode::Code,
                instruction_epoch: 10
            }
        );

        let mut cfg = harness_runtime::desktop::DesktopConfig::default();
        cfg.cwd = cwd.clone();
        cfg.lingxi_home = home;
        cfg.session_persistence = false;
        let captured = crate::init::capture_tui_resume(&mut cfg, session_id, None)
            .await
            .unwrap();
        let resumed = orchestrator::ConversationOrchestrator::new(
            orchestrator::OrchestratorConfig::default(),
            Arc::new(orchestrator::test_support::MockApiClient::new(vec![])),
            Arc::new(tool_api::ToolRegistry::new()),
            orchestrator::test_support::noop_hook_executor(),
            Arc::new(orchestrator::test_support::NoOpPermissionGate),
            Arc::new(orchestrator::test_support::MockOutputStream::new()),
            Arc::new(orchestrator::test_support::StaticMemoryProvider::empty()),
            cwd,
        )
        .with_session_id(lingxi_core::types::SessionId::from_uuid(session_id))
        .with_conversation_mode(ConversationMode::Code);
        resumed
            .restore_captured_session_history(
                session_id,
                &captured.messages,
                &captured.entries,
                false,
                permission::PermissionMode::Default,
                ConversationMode::Code,
                &cfg.session_resume_snapshot.as_ref().unwrap().content,
            )
            .await
            .unwrap();
        let session = resumed.session();
        let state = session.lock().await;
        assert_eq!(state.history.len(), 3);
        let ids = state
            .model_context_history()
            .iter()
            .map(ConversationMessage::id)
            .collect::<std::collections::HashSet<_>>();
        assert!(!ids.contains(&old_id));
        assert!(!ids.contains(&future_id));
        assert!(ids.contains(&user_id));
    }

    #[tokio::test]
    async fn exhausted_fork_epoch_fails_before_filesystem_publication() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("config");
        let runtime = root.path().join("runtime");
        let forker = CliBgSessionForker::new(
            home.clone(),
            runtime.clone(),
            Default::default(),
            permission::PermissionMode::Default,
        );
        let conversation = BgSessionSnapshot {
            mode: ConversationMode::Code,
            instruction_epoch: (1_u64 << 53) - 1,
            ..Default::default()
        };
        let error = forker
            .fork_to_background(&conversation, None, &"".into(), "model")
            .await
            .unwrap_err();
        assert!(
            matches!(error, BgForkError::Snapshot(message) if message.contains("epoch is exhausted"))
        );
        assert!(!home.exists());
        assert!(!runtime.exists());
    }

    #[tokio::test]
    async fn background_snapshot_cold_resume_preserves_context_and_permissions() {
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().join("workspace");
        std::fs::create_dir_all(&cwd).unwrap();
        let config_home = root.path().join("config");
        let cwd_string = cwd.to_string_lossy().into_owned();
        let options = crate::background_launch::BackgroundLaunchOptions {
            allowed_tools: Some(vec!["Read".into()]),
            disallowed_tools: Some(vec!["Write".into()]),
            tools: Some(vec!["Read".into()]),
            add_dir: Some(vec![root.path().join("authorized")]),
            permission_mode: Some("plan".into()),
            ..Default::default()
        };
        let forker = CliBgSessionForker::new(
            config_home.clone(),
            root.path().join("runtime"),
            options,
            permission::PermissionMode::Plan,
        );
        let before = forker.launch_context(permission::PermissionMode::Plan);

        // Exercise both transition directions. The epoch is carried even when
        // no foreground pending selection belongs to this independent copy.
        for (mode, epoch) in [(ConversationMode::Chat, 1), (ConversationMode::Code, 2)] {
            let session_id = uuid::Uuid::new_v4();
            let old_id = MessageId::new();
            let passive_old_id = MessageId::new();
            let user_id = MessageId::new();
            let current_id = MessageId::new();
            let excluded_attachment_id = MessageId::new();
            let visible_attachment_id = MessageId::new();
            let old_origin =
                Source::parse(r#"{"type":"skill_activation","context_epoch":0}"#).unwrap();
            let current_origin = Source::parse(&format!(
                r#"{{"type":"skill_activation","context_epoch":{epoch}}}"#
            ))
            .unwrap();
            let old = ConversationMessage::user_meta(old_id, "old instruction".into())
                .with_origin(old_origin.clone());
            let passive_old =
                ConversationMessage::user_meta(passive_old_id, "other old instruction".into())
                    .with_origin(old_origin);
            // Identical prose without host origin remains ordinary user input.
            let user = ConversationMessage::user(user_id, "old instruction".into());
            let current = ConversationMessage::user_meta(current_id, "current instruction".into())
                .with_origin(current_origin);
            let excluded_attachment =
                ConversationMessage::user_meta(excluded_attachment_id, "same evidence".into());
            let visible_attachment =
                ConversationMessage::user_meta(visible_attachment_id, "same evidence".into());
            let attachment = Source::parse(r#"{"type":"environment","snapshot":{}}"#).unwrap();
            let conversation = BgSessionSnapshot {
                history: vec![
                    old,
                    passive_old,
                    user,
                    current,
                    excluded_attachment,
                    visible_attachment,
                ],
                model_context_excluded_messages: [old_id, excluded_attachment_id]
                    .into_iter()
                    .collect(),
                mode,
                instruction_epoch: epoch,
                model_reminder_attachments: vec![
                    (excluded_attachment_id, attachment.clone()),
                    (visible_attachment_id, attachment),
                ],
                ..Default::default()
            };
            let fs: Arc<dyn FileSystem> = Arc::new(HostFileSystem::new(cwd.clone()));
            let path = session::jsonl::path::session_path(
                &config_home,
                &cwd_string,
                &session_id.to_string(),
            );
            let writer = Arc::new(session::jsonl::writer::JsonlWriter::new(
                path.clone(),
                fs.clone(),
            ));
            write_background_snapshot(
                &writer,
                &conversation,
                &session_id.to_string(),
                &cwd_string,
                "test",
                "model",
            )
            .await
            .unwrap();
            let raw = std::fs::read_to_string(&path).unwrap();
            let first: serde_json::Value =
                serde_json::from_str(raw.lines().next().unwrap()).unwrap();
            assert_eq!(first["type"], "fixed-session-mode");
            assert_eq!(first["sessionId"], session_id.to_string());
            assert_eq!(first["sessionMode"], mode.as_str());
            assert!(raw.contains("old instruction"));
            assert!(raw.contains("isModelContextExcluded"));

            let reopened = Arc::new(session::jsonl::writer::JsonlWriter::new(
                path.clone(),
                fs.clone(),
            ));
            assert_eq!(
                reopened.persisted_fixed_session_profile().await.unwrap(),
                Some(FixedSessionProfile {
                    mode,
                    instruction_epoch: epoch
                })
            );
            let mut cfg = harness_runtime::desktop::DesktopConfig::default();
            cfg.cwd = cwd.clone();
            cfg.lingxi_home = config_home.clone();
            let captured = crate::init::capture_tui_resume(&mut cfg, session_id, None)
                .await
                .unwrap();
            let canonical_path = path.canonicalize().unwrap();
            assert_eq!(
                cfg.session_transcript_path.as_deref(),
                Some(canonical_path.as_path())
            );
            let source = &cfg.session_resume_snapshot.as_ref().unwrap().content;
            let resumed = orchestrator::ConversationOrchestrator::new(
                orchestrator::OrchestratorConfig::default(),
                Arc::new(orchestrator::test_support::MockApiClient::new(vec![])),
                Arc::new(tool_api::ToolRegistry::new()),
                orchestrator::test_support::noop_hook_executor(),
                Arc::new(orchestrator::test_support::NoOpPermissionGate),
                Arc::new(orchestrator::test_support::MockOutputStream::new()),
                Arc::new(orchestrator::test_support::StaticMemoryProvider::empty()),
                cwd.clone(),
            )
            .with_session_id(lingxi_core::types::SessionId::from_uuid(session_id))
            .with_conversation_mode(mode)
            .with_jsonl_writer(reopened);
            resumed
                .restore_captured_session_history(
                    session_id,
                    &captured.messages,
                    &captured.entries,
                    false,
                    permission::PermissionMode::Plan,
                    mode,
                    source,
                )
                .await
                .unwrap();
            let mut worker_options = before.options.clone().unwrap();
            worker_options.preserve_admitted_context = true;
            let worker_options: crate::background_launch::BackgroundLaunchOptions =
                serde_json::from_str(&serde_json::to_string(&worker_options).unwrap()).unwrap();
            assert!(worker_options.preserve_admitted_context);
            // This is the state desktop assembly exposes before SessionStart.
            let restored = resumed.conversation_mode().await;
            assert_eq!(restored.mode, mode);
            let session = resumed.session();
            let state = session.lock().await;
            assert_eq!(state.history.len(), 6, "audit transcript is retained");
            let ids = state
                .model_context_history()
                .iter()
                .map(ConversationMessage::id)
                .collect::<std::collections::HashSet<_>>();
            assert!(
                !ids.contains(&old_id),
                "explicit model exclusion survives serialization"
            );
            assert!(
                !ids.contains(&passive_old_id),
                "the restored epoch retires older skills"
            );
            assert!(ids.contains(&user_id), "user evidence is retained");
            assert!(ids.contains(&current_id), "current skill is retained");
            assert!(!ids.contains(&excluded_attachment_id));
            assert!(ids.contains(&visible_attachment_id));
            drop(state);
            drop(session);
            drop(resumed);

            // Exercise the worker's actual runtime assembly and its mounted
            // profile boundary with the same serialized launch options.
            cfg.session_id_override = Some(session_id.to_string());
            assert_eq!(cfg.initial_conversation_mode, Some(mode));
            cfg.native_settings_agent_context =
                lingxi_core::host::NativeSettingsAgentContext::Subagent {
                    is_main_session: true,
                };
            let runtime = crate::init::build_runtime_from_config(
                cfg,
                Arc::new(orchestrator::test_support::MockOutputStream::new()),
            )
            .await
            .unwrap();
            assert_eq!(runtime.orchestrator.conversation_mode().await.mode, mode);
            let mounted = runtime.orchestrator.session();
            assert!(mounted
                .lock()
                .await
                .model_context_history()
                .iter()
                .any(|row| row.id() == current_id));
            drop(mounted);
            let shutdown = runtime.session_lifecycle.shutdown_and_drain().await;
            assert!(shutdown.complete, "{:?}", shutdown.errors);
        }

        let after = forker.launch_context(permission::PermissionMode::Plan);
        assert_eq!(after.options, before.options);
        assert_eq!(
            after.resolved_permission_mode,
            before.resolved_permission_mode
        );
    }

    #[tokio::test]
    async fn nonpersistent_cold_resume_restores_captured_profile_without_writes() {
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().join("workspace");
        std::fs::create_dir_all(&cwd).unwrap();
        let config_home = root.path().join("config");
        let cwd_string = cwd.to_string_lossy().into_owned();
        let session_id = uuid::Uuid::new_v4();
        let old_id = MessageId::new();
        let current_id = MessageId::new();
        let user_id = MessageId::new();
        let excluded_id = MessageId::new();
        let old = ConversationMessage::user_meta(old_id, "same evidence".into()).with_origin(
            Source::parse(r#"{"type":"skill_activation","context_epoch":1}"#).unwrap(),
        );
        let current = ConversationMessage::user_meta(current_id, "current instruction".into())
            .with_origin(
                Source::parse(
                    r#"{"type":"skill_activation","context_epoch":2,"skill_name":"current"}"#,
                )
                .unwrap(),
            );
        let user = ConversationMessage::user(user_id, "same evidence".into());
        let excluded = ConversationMessage::user_meta(excluded_id, "excluded evidence".into());
        let conversation = BgSessionSnapshot {
            history: vec![old, current, user, excluded],
            model_context_excluded_messages: [excluded_id].into_iter().collect(),
            mode: ConversationMode::Chat,
            instruction_epoch: 2,
            ..Default::default()
        };
        let fs: Arc<dyn FileSystem> = Arc::new(HostFileSystem::new(cwd.clone()));
        let path =
            session::jsonl::path::session_path(&config_home, &cwd_string, &session_id.to_string());
        let writer = session::JsonlWriter::new(path.clone(), fs);
        write_background_snapshot(
            &writer,
            &conversation,
            &session_id.to_string(),
            &cwd_string,
            "test",
            "model",
        )
        .await
        .unwrap();
        let before = std::fs::read(&path).unwrap();
        let first: serde_json::Value =
            serde_json::from_slice(before.split(|byte| *byte == b'\n').next().unwrap()).unwrap();
        assert_eq!(first["sessionMode"], "chat");
        assert_eq!(first["instructionEpoch"], 2);
        let mut cfg = harness_runtime::desktop::DesktopConfig::default();
        cfg.cwd = cwd.clone();
        cfg.lingxi_home = config_home.clone();
        cfg.session_persistence = false;
        let captured = crate::init::capture_tui_resume(&mut cfg, session_id, None)
            .await
            .unwrap();
        assert!(cfg.session_writer_lease.is_none());
        assert!(cfg.session_resume_cost.is_none());
        let source = &cfg.session_resume_snapshot.as_ref().unwrap().content;
        let profile =
            session::JsonlWriter::fixed_session_mode_from_source(source, &session_id.to_string())
                .unwrap();
        let resumed = orchestrator::ConversationOrchestrator::new(
            orchestrator::OrchestratorConfig::default(),
            Arc::new(orchestrator::test_support::MockApiClient::new(vec![])),
            Arc::new(tool_api::ToolRegistry::new()),
            orchestrator::test_support::noop_hook_executor(),
            Arc::new(orchestrator::test_support::NoOpPermissionGate),
            Arc::new(orchestrator::test_support::MockOutputStream::new()),
            Arc::new(orchestrator::test_support::StaticMemoryProvider::empty()),
            cwd,
        )
        .with_session_id(lingxi_core::types::SessionId::from_uuid(session_id))
        .with_conversation_mode(ConversationMode::Chat);
        resumed
            .restore_captured_session_history(
                session_id,
                &captured.messages,
                &captured.entries,
                false,
                permission::PermissionMode::Default,
                profile,
                source,
            )
            .await
            .unwrap();
        let mode = resumed.conversation_mode().await;
        assert_eq!(mode.mode, ConversationMode::Chat);
        let session = resumed.session();
        let state = session.lock().await;
        assert_eq!(state.history.len(), 4);
        let ids = state
            .model_context_history()
            .iter()
            .map(ConversationMessage::id)
            .collect::<std::collections::HashSet<_>>();
        assert!(!ids.contains(&old_id));
        assert!(!ids.contains(&excluded_id));
        assert!(ids.contains(&current_id));
        assert!(ids.contains(&user_id));
        assert!(
            matches!(state.history.iter().find(|row| row.id() == current_id),
            Some(ConversationMessage::User { origin: Some(origin), .. })
                if origin.number_at("/context_epoch") == Some(2.0))
        );
        drop(state);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(!config_home.join("sessions").exists());
    }
}
