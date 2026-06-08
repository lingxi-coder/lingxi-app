//! Orchestrator-side [`hooks::CwdChangedFirer`] implementation.
//!
//! Counterpart to [`OrchestratorTaskCreatedFirer`](crate::OrchestratorTaskCreatedFirer)
//! and [`OrchestratorTaskCompletedFirer`](crate::OrchestratorTaskCompletedFirer).
//! The `tool-shell` crate is a LEAF: [`BashTool`](../../tools/shell/src/bash.rs)
//! owns the persistent shell cwd but has no access to a live hook executor, and
//! the orchestrator depends on `hooks` but NOT on `tool-shell`. The firer trait
//! therefore lives in `hooks` (both crates can name it); this adapter closes the
//! seam from the orchestrator side: it owns the engine's
//! `Arc<hooks::HookExecutorImpl>` (`orch.hooks`) plus the engine cwd, translates
//! a [`hooks::CwdChangedFire`] into a [`hooks::HookEvent::CwdChanged`], and fires
//! the registry best-effort.
//!
//! Parity: reproduces claude-code's `onCwdChangedForHooks(oldCwd, newCwd)`
//! (`utils/Shell.ts:409`, fired from the `pwd -P` readback) → `executeCwdChangedHooks`
//! (`utils/hooks.ts:4260`) — the wire payload (`CwdChangedHookInput`,
//! `types/hooks.ts:146`) carries `old_cwd` / `new_cwd`.
//!
//! Best-effort: [`HookExecutorImpl::execute`] never errors out, so a failing or
//! absent `CwdChanged` hook degrades to a no-op and never breaks the cwd update
//! — matching the `TaskCreated` / `TaskCompleted` arms.
//!
//! Wiring: the composition root (`engine-desktop`) builds an
//! [`OrchestratorCwdChangedFirer`] over the SAME `Arc<HookExecutorImpl>` it hands
//! the orchestrator, then injects it via `BashTool::with_cwd_changed_firer` where
//! the desktop shell tools are registered. `engine-mobile` never registers the
//! shell tools (`tool_shell::register_all` is desktop-only), so the mobile path
//! keeps the plain `BashTool::new(ctx)` with the firer `None`.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use hooks::cwd_changed_firer::{CwdChangedFire, CwdChangedFirer};
use hooks::events::HookEvent;
use hooks::registry::HookContext;
use hooks::HookExecutorImpl;

/// Adapts the engine's hook executor to the `tool-shell` crate's `CwdChanged`
/// firer seam.
pub struct OrchestratorCwdChangedFirer {
    /// The SAME executor the orchestrator fires its other hooks through.
    hooks: Arc<HookExecutorImpl>,
    /// Engine cwd, threaded into the `CwdChanged` hook payload `cwd` field (the
    /// session base dir) and the per-hook Command-arm `CLAUDE_PROJECT_DIR`
    /// fallback. This is distinct from the fire's `old`/`new` shell cwd, which
    /// becomes the `old_cwd` / `new_cwd` payload fields.
    cwd: PathBuf,
}

impl OrchestratorCwdChangedFirer {
    /// Build a firer over the shared hook executor and engine cwd. Pass the SAME
    /// `Arc<HookExecutorImpl>` handed to the orchestrator so the `CwdChanged`
    /// hook rides the identical registry / async / sandbox plumbing.
    #[must_use]
    pub fn new(hooks: Arc<HookExecutorImpl>, cwd: PathBuf) -> Self {
        Self { hooks, cwd }
    }
}

#[async_trait]
impl CwdChangedFirer for OrchestratorCwdChangedFirer {
    async fn fire(&self, fire: CwdChangedFire) {
        // Map the fire's old/new shell cwd onto the `HookEvent::CwdChanged`
        // variant (`old` / `new`); the envelope builder serializes these as the
        // wire `old_cwd` / `new_cwd` (executor.rs build_lifecycle_envelope_body).
        let event = HookEvent::CwdChanged {
            old: fire.old,
            new: fire.new,
        };
        // Context-light: a cwd change has no live per-turn session here, so we
        // thread only the engine cwd (also the CLAUDE_PROJECT_DIR fallback).
        // Everything else defaults — matching the `OrchestratorTaskCreatedFirer`.
        let ctx = HookContext {
            cwd: self.cwd.clone(),
            ..Default::default()
        };
        // Best-effort: the executor never errors out of `execute`, so a
        // misbehaving / absent hook degrades to a no-op and never breaks the
        // caller's cwd update. The aggregate result is intentionally dropped —
        // `CwdChanged` is observational.
        let _ = self.hooks.execute(event, ctx).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::noop_hook_executor;
    use std::sync::Mutex;

    use hooks::definition::{HookDefinition, HookExecutor as DefHookExecutor};
    use hooks::events::HookEventType;
    use hooks::registry::HookRegistry;
    use hooks::response::{HookOutcome, HookResult};
    use hooks::{BuiltinHookHandler, HookSource};

    #[tokio::test]
    async fn no_matching_hook_is_a_noop_fire() {
        // An executor with an empty registry never intervenes => the fire is a
        // silent no-op (the "no hook registered" contract). Must not panic/hang.
        let firer = OrchestratorCwdChangedFirer::new(noop_hook_executor(), PathBuf::from("/work"));
        firer
            .fire(CwdChangedFire {
                old: PathBuf::from("/work"),
                new: PathBuf::from("/work/sub"),
            })
            .await;
    }

    // Inert HTTP / runtime stubs: the Builtin hook arm dispatches in-process,
    // so neither is ever called. Mirrors `test_support::noop_hook_executor`.
    struct UnusedHttp;
    #[async_trait]
    impl traits::HttpTransport for UnusedHttp {
        async fn request(
            &self,
            _req: protocol::HttpRequest,
        ) -> Result<protocol::HttpResponse, traits::HttpError> {
            Err(traits::HttpError::InvalidRequest("unused".into()))
        }
        async fn stream_sse(
            &self,
            _req: protocol::HttpRequest,
        ) -> Result<traits::http::SseStream, traits::HttpError> {
            Err(traits::HttpError::InvalidRequest("unused".into()))
        }
    }
    struct UnusedRuntime;
    #[async_trait]
    impl traits::RuntimeSpawner for UnusedRuntime {
        async fn spawn(
            &self,
            _name: &str,
            _task: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>,
        ) -> Result<traits::BackgroundTaskHandle, traits::RuntimeError> {
            Err(traits::RuntimeError::Internal("unused".into()))
        }
        async fn sleep(&self, _duration: std::time::Duration) {}
        async fn cancel(
            &self,
            _handle: &traits::BackgroundTaskHandle,
        ) -> Result<(), traits::RuntimeError> {
            Ok(())
        }
    }

    /// Records the `(old, new)` it is dispatched, proving the firer's
    /// `CwdChangedFire` reaches the executor as a `HookEvent::CwdChanged`.
    struct RecordingBuiltin {
        seen: Arc<Mutex<Option<(PathBuf, PathBuf)>>>,
    }
    #[async_trait]
    impl BuiltinHookHandler for RecordingBuiltin {
        async fn handle(&self, event: &HookEvent, _ctx: &HookContext) -> HookResult {
            if let HookEvent::CwdChanged { old, new } = event {
                *self.seen.lock().unwrap() = Some((old.clone(), new.clone()));
            }
            HookResult {
                outcome: HookOutcome::Success,
                stdout: String::new(),
                stderr: String::new(),
                exit_code: Some(0),
                response: None,
            }
        }
        fn id(&self) -> &str {
            "cwd-changed-recorder"
        }
    }

    #[tokio::test]
    async fn fire_maps_to_delivered_cwd_changed_event() {
        // A Builtin hook subscribed to `CwdChanged` routed to a recording
        // handler: firing the orchestrator firer must deliver a
        // `HookEvent::CwdChanged { old, new }` carrying the fire's paths.
        let seen: Arc<Mutex<Option<(PathBuf, PathBuf)>>> = Arc::new(Mutex::new(None));
        let handler = Arc::new(RecordingBuiltin { seen: seen.clone() });

        let mut registry = HookRegistry::new();
        registry.register(HookDefinition {
            id: protocol::HookId::new(),
            name: "cwd-changed".into(),
            events: vec![HookEventType::CwdChanged],
            if_condition: None,
            executor: DefHookExecutor::Builtin {
                handler_id: "cwd-changed-recorder".into(),
            },
            source: HookSource::Skill,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
        });

        // Build a fresh executor over the populated registry. The Builtin arm
        // dispatches in-process, so the http/runtime stubs are inert.
        let reg = Arc::new(tokio::sync::RwLock::new(registry));
        let mut exec = HookExecutorImpl::new(
            reg,
            Arc::new(UnusedHttp) as Arc<dyn traits::HttpTransport>,
            Arc::new(UnusedRuntime) as Arc<dyn traits::RuntimeSpawner>,
        );
        exec.register_builtin(handler);

        let firer = OrchestratorCwdChangedFirer::new(Arc::new(exec), PathBuf::from("/work"));
        firer
            .fire(CwdChangedFire {
                old: PathBuf::from("/work/old"),
                new: PathBuf::from("/work/new"),
            })
            .await;

        let got = seen.lock().unwrap().clone();
        assert_eq!(
            got,
            Some((PathBuf::from("/work/old"), PathBuf::from("/work/new"))),
            "OrchestratorCwdChangedFirer must deliver HookEvent::CwdChanged(old,new)"
        );
    }
}
