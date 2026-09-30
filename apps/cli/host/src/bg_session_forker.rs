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
use lingxi_core::host::bg_session_forker::{BgForkError, BgSessionForker};
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
    /// Foreground-resolved mode that already passed the bypass safety guard.
    resolved_permission_mode: permission::PermissionMode,
}

impl CliBgSessionForker {
    /// Construct a forker over the resolved config-home + daemon runtime dir.
    #[must_use]
    pub fn new(
        config_home: PathBuf,
        runtime_dir: PathBuf,
        launch_options: crate::background_launch::BackgroundLaunchOptions,
        resolved_permission_mode: permission::PermissionMode,
    ) -> Self {
        Self {
            task_registry: RwLock::new(None),
            config_home,
            runtime_dir,
            version: env!("CARGO_PKG_VERSION").to_string(),
            launch_options,
            resolved_permission_mode,
        }
    }

    fn launch_context(&self) -> crate::background_dispatch::ForkLaunchContext {
        crate::background_dispatch::ForkLaunchContext {
            options: Some(self.launch_options.clone()),
            resolved_permission_mode: Some(self.resolved_permission_mode),
            ..crate::background_dispatch::ForkLaunchContext::default()
        }
    }

    async fn fork_with_handoff(
        &self,
        history: &[lingxi_core::types::ConversationMessage],
        system_prompt: Option<Arc<str>>,
        prompt: &str,
        model: &str,
        handoff: Option<&lingxi_core::host::BackgroundingSnapshot>,
    ) -> Result<String, BgForkError> {
        // Resolve the LIVE cwd at fork time (a Bash `cd` may have moved it since
        // boot) so the snapshot path and the recorded job cwd agree.
        let cwd_pb = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let cwd = cwd_pb.display().to_string();

        // 1. Mint the new background session id.
        let new_session_id = uuid::Uuid::new_v4().to_string();

        // 2. SNAPSHOT the conversation into the new session's transcript.
        let session_path =
            session::jsonl::path::session_path(&self.config_home, &cwd, &new_session_id);
        let lines = orchestrator::bg_snapshot::history_to_jsonl_lines(
            history,
            &new_session_id,
            &cwd,
            &self.version,
            model,
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
        let mut launch_context = self.launch_context();
        launch_context.model = Some(model.to_string());
        launch_context.handoff = handoff.cloned();
        let registry = self
            .task_registry
            .read()
            .expect("background registry lock")
            .clone();
        if handoff.is_some() {
            if let Some(registry) = &registry {
                launch_context.shell_handoff = registry
                    .export_shell_handoff()
                    .await
                    .map_err(|error| BgForkError::Dispatch(error.to_string()))?;
            }
        }
        if let Some(system_prompt) = system_prompt {
            launch_context.system_prompt = Some(system_prompt.to_string());
        }
        let short = crate::background_dispatch::dispatch_forked_session_with_context(
            &self.config_home,
            &self.runtime_dir,
            &cwd,
            &new_session_id,
            prompt,
            &launch_context,
        );
        let short = match short {
            Ok(short) => short,
            Err(error) => {
                if let Some(registry) = &registry {
                    if error.kind() == std::io::ErrorKind::WouldBlock {
                        return Err(BgForkError::Dispatch(error.to_string()));
                    }
                    registry
                        .rollback_shell_handoff(&launch_context.shell_handoff)
                        .await
                        .map_err(|rollback| {
                            BgForkError::Dispatch(format!(
                                "{error}; shell export recovery failed: {rollback}"
                            ))
                        })?;
                }
                return Err(BgForkError::Dispatch(error.to_string()));
            }
        };

        if let Some(registry) = &registry {
            crate::shell_handoff::finish_source(
                &self.config_home,
                &short,
                registry.as_ref(),
                &launch_context.shell_handoff,
            )
            .await
            .map_err(BgForkError::Dispatch)?;
        }

        // 4. The live-session system line. No oracle string is recoverable — the
        //    2.1.212 `vAd` command has no `load` handler (the TUI special
        //    dispatcher renders this), so this is grounded on the minted short id,
        //    NOT byte-verified.
        Ok(format!(
            "Copied conversation into a new background session ({short})."
        ))
    }
}

#[async_trait]
impl BgSessionForker for CliBgSessionForker {
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
        history: &[lingxi_core::types::ConversationMessage],
        system_prompt: Option<Arc<str>>,
        prompt: &str,
        model: &str,
    ) -> Result<String, BgForkError> {
        self.fork_with_handoff(history, system_prompt, prompt, model, None)
            .await
    }

    async fn background_conversation(
        &self,
        history: &[lingxi_core::types::ConversationMessage],
        system_prompt: Option<Arc<str>>,
        prompt: &str,
        model: &str,
        snapshot: &lingxi_core::host::BackgroundingSnapshot,
    ) -> Result<String, BgForkError> {
        self.fork_with_handoff(history, system_prompt, prompt, model, Some(snapshot))
            .await
    }

    async fn resume_to_background(&self, session_id: &str) -> Result<String, BgForkError> {
        // The chosen session ALREADY exists on disk at its standard transcript
        // path (`list_resumable_sessions` enumerated it), so — unlike the fork
        // path — there is NO snapshot to write: dispatch a detached daemon that
        // resumes `session_id` directly via the same `Launch::Resume` machinery.
        let cwd_pb = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let cwd = cwd_pb.display().to_string();
        let short = crate::background_dispatch::dispatch_resumed_session_with_context(
            &self.config_home,
            &self.runtime_dir,
            &cwd,
            session_id,
            &self.launch_context(),
        )
        .map_err(|e| BgForkError::Dispatch(e.to_string()))?;
        // Same grounding caveat as `fork_to_background`: the 2.1.212 resume-as-bg
        // dispatch has no recoverable oracle line (the TUI special dispatcher
        // renders it), so this is grounded on the minted short id, NOT byte-verified.
        Ok(format!(
            "Resumed session into a new background session ({short})."
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let context = forker.launch_context();
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
