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
use lingxi_core::host::bg_session_forker::lifecycle::BackgroundLaunchLifecycle;
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
    registry: Option<Arc<dyn lingxi_core::host::task_registry::TaskRegistryHandle>>,
}

fn pin_resume_source(
    config_home: &std::path::Path,
    cwd: &std::path::Path,
    session_id: &str,
    context: &mut crate::background_dispatch::ForkLaunchContext,
) -> Result<(), BgForkError> {
    uuid::Uuid::parse_str(session_id)
        .map_err(|error| BgForkError::Dispatch(format!("invalid session id: {error}")))?;
    let source = crate::background_dispatch::locate_session_transcript(
        config_home,
        &cwd.display().to_string(),
        session_id,
    )
    .map_err(|error| BgForkError::Dispatch(error.to_string()))?;
    context.transcript_path = Some(source.to_string_lossy().into_owned());
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
        // Claim before the first I/O. The worker retains this claim even when
        // its UI caller is cancelled, so shutdown drains the real publication.
        let claim = self.lifecycle.claim()?;
        let (cwd, context) = self.launch_identity()?;
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
            registry: self
                .task_registry
                .read()
                .expect("background registry lock")
                .clone(),
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
            registry,
        } = publication;
        let cwd = cwd_pb.display().to_string();

        // 1. Mint the new background session id.
        let new_session_id = uuid::Uuid::new_v4().to_string();

        // 2. SNAPSHOT the conversation into the new session's transcript.
        let session_path = session::jsonl::path::session_path(&config_home, &cwd, &new_session_id);
        let lines = orchestrator::bg_snapshot::history_to_jsonl_lines(
            &conversation,
            &new_session_id,
            &cwd,
            &version,
            &model,
        );
        let fs: Arc<dyn FileSystem> = Arc::new(HostFileSystem::new(cwd_pb.clone()));
        let writer = session::jsonl::writer::JsonlWriter::new(session_path, fs);
        for line in &lines {
            writer
                .append(line)
                .await
                .map_err(|e| BgForkError::Snapshot(e.to_string()))?;
        }
        // 3. Dispatch a detached daemon worker that resumes the copied session.
        launch_context.model = Some(model.to_string());
        launch_context.handoff = handoff.clone();
        if handoff.is_some() {
            if let Some(registry) = &registry {
                launch_context.shell_handoff = registry
                    .export_shell_handoff()
                    .await
                    .map_err(|error| BgForkError::Dispatch(error.to_string()))?;
            }
        }
        if let Some(system_prompt) = system_prompt {
            launch_context.system_prompt = Some(system_prompt.as_ref().clone());
        }
        let short = crate::background_dispatch::dispatch_forked_session_with_context(
            &config_home,
            &runtime_dir,
            &cwd,
            &new_session_id,
            &prompt,
            &launch_context,
        );
        let short = match short {
            Ok(short) => short,
            Err(error) => {
                if let Some(registry) = &registry {
                    if error.kind() == std::io::ErrorKind::WouldBlock {
                        return Err(BgForkError::OwnershipUncertain(error.to_string()));
                    }
                    registry
                        .rollback_shell_handoff(&launch_context.shell_handoff)
                        .await
                        .map_err(|rollback| {
                            BgForkError::OwnershipUncertain(format!(
                                "{error}; shell export recovery failed: {rollback}"
                            ))
                        })?;
                }
                return Err(BgForkError::Dispatch(error.to_string()));
            }
        };

        if let Some(registry) = &registry {
            crate::shell_handoff::finish_source(
                &config_home,
                &short,
                registry.as_ref(),
                &launch_context.shell_handoff,
            )
            .await
            .map_err(BgForkError::from)?;
        }

        // 4. The live-session system line. No oracle string is recoverable — the
        //    2.1.212 `vAd` command has no `load` handler (the TUI special
        //    dispatcher renders this), so this is grounded on the minted short id,
        //    NOT byte-verified.
        Ok(format!("Copied conversation into a new background session ({short}).").into())
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
        self.fork_with_handoff(conversation, system_prompt, prompt, model, None)
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
        let (cwd_pb, mut launch_context) = self.launch_identity()?;
        // Freeze the selected catalog path while this foreground claim is
        // live. If it disappears, dispatch fails rather than re-resolving the
        // UUID in another project. This does not snapshot the file contents.
        pin_resume_source(&self.config_home, &cwd_pb, session_id, &mut launch_context)?;
        let config_home = self.config_home.clone();
        let runtime_dir = self.runtime_dir.clone();
        let session_id = session_id.to_string();
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
        std::fs::write(&source, "original").unwrap();
        let mut context = crate::background_dispatch::ForkLaunchContext::default();
        pin_resume_source(&home, &cwd, &session_id, &mut context).unwrap();
        assert_eq!(
            std::path::PathBuf::from(context.transcript_path.as_ref().unwrap()),
            source.canonicalize().unwrap()
        );
        std::fs::rename(&source, source.with_extension("moved")).unwrap();
        std::fs::write(replacement, "different source").unwrap();
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
        assert!(!home.join("jobs").exists());
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
        forker.lifecycle().close_and_drain().await.unwrap();
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
}
