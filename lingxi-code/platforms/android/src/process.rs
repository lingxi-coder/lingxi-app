//! [`traits::ProcessRunner`] for Android. P1 ships the SECURITY INVARIANTS
//! only — execution stays disabled until the P0a gate passes and the P2 plan
//! lands the in-engine `minijail_run_pid_pipes` path.

use std::sync::Arc;

use async_trait::async_trait;
use traits::{
    ProcessError, ProcessHandle, ProcessOutput, ProcessRunner, SandboxBackend, SandboxedCommand,
    SandboxedTag,
};

use crate::policy::AndroidSandboxPlan;

/// Android process runner. Holds no state in P1.
#[derive(Default)]
pub struct AndroidMinijailProcessRunner;

impl AndroidMinijailProcessRunner {
    /// Construct.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Validate the three security invariants and return the admitted plan.
    ///
    /// # Why the runner re-checks what the type system cannot
    ///
    /// `__new_sandboxed_with_plan` does not constrain tag↔plan consistency:
    /// any caller can attach any plan to any tag (or no plan). The runner is
    /// therefore the last-chance enforcement point (defense-in-depth per spec
    /// r3 §`AndroidMinijailProcessRunner` and prior review):
    ///
    /// 1. `BypassAuditedWithReason` — the desktop `BashTool` shape dies here
    ///    by construction; this is the ONLY audit trace if it was silently
    ///    dropped upstream.
    /// 2. `Wrapped` with a backend ≠ `AndroidMinijail` — a foreign plan
    ///    (e.g. macOS `sandbox-exec`) must never execute on Android.
    /// 3. Plan missing or not downcasting to `AndroidSandboxPlan` — the plan
    ///    field is `Option` so `prepare()` could have been bypassed; downcast
    ///    failure means the wrong backend minted the handle.
    fn admitted_plan(cmd: &SandboxedCommand) -> Result<Arc<AndroidSandboxPlan>, ProcessError> {
        match cmd.tag() {
            SandboxedTag::BypassAuditedWithReason { reason } => {
                let msg =
                    "bypass-audited commands cannot execute on Android (security invariant #1)";
                tracing::warn!(bypass_reason = %reason, "android runner rejected bypass-audited command");
                return Err(ProcessError::MalformedSandboxPlan(msg.into()));
            }
            SandboxedTag::Wrapped { backend } if *backend != SandboxBackend::AndroidMinijail => {
                let msg = format!("foreign sandbox backend {backend:?} (security invariant #2)");
                tracing::warn!(rejection = %msg, "android runner rejected command");
                return Err(ProcessError::MalformedSandboxPlan(msg));
            }
            SandboxedTag::Wrapped { .. } => {}
        }
        let handle = cmd.backend_plan().ok_or_else(|| {
            let msg = "android plan missing from SandboxedCommand (security invariant #3)";
            tracing::warn!(rejection = msg, "android runner rejected command");
            ProcessError::MalformedSandboxPlan(msg.into())
        })?;
        handle.downcast::<AndroidSandboxPlan>().ok_or_else(|| {
            let msg = "backend plan failed to downcast to AndroidSandboxPlan (security invariant #3) — wrong backend minted the handle, or the plan was wrapped pre-Arc'd";
            tracing::warn!(rejection = msg, "android runner rejected command");
            ProcessError::MalformedSandboxPlan(msg.into())
        })
    }
}

#[async_trait]
impl ProcessRunner for AndroidMinijailProcessRunner {
    async fn run(&self, cmd: &SandboxedCommand) -> Result<ProcessOutput, ProcessError> {
        // P2: plan.argv/plan.env are authoritative over cmd.inner() (inner mirrors them for audit only) — spawn from the plan.
        let _plan = Self::admitted_plan(cmd)?;
        // P2 (gated on P0a): spawn_blocking → minijail_run_pid_pipes.
        Err(ProcessError::Unsupported)
    }

    async fn spawn_background(
        &self,
        cmd: &SandboxedCommand,
    ) -> Result<ProcessHandle, ProcessError> {
        // Invariants must hold even for background tasks — bypass/foreign-backend
        // commands are rejected here, producing an audit trace (MalformedSandboxPlan)
        // rather than silently returning Unsupported.
        let _plan = Self::admitted_plan(cmd)?;
        // Spec non-goal 5: no background shell tasks in v1.
        Err(ProcessError::Unsupported)
    }

    async fn kill(&self, _handle: &ProcessHandle) -> Result<(), ProcessError> {
        Err(ProcessError::Unsupported)
    }

    fn is_available(&self) -> bool {
        // Flips with the P2 execution plan; false keeps registration gate #5
        // closed so no tool registers against the P1 skeleton.
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use traits::{
        BackendPlanHandle, ProcessCommand, ProcessError, ProcessRunner, SandboxBackend,
        SandboxedCommand, SandboxedTag,
    };

    fn cmd() -> ProcessCommand {
        ProcessCommand {
            command: "/system/bin/sh".into(),
            args: vec![],
            cwd: None,
            env: HashMap::new(),
            timeout: None,
            stdin: None,
        }
    }

    fn runner() -> AndroidMinijailProcessRunner {
        AndroidMinijailProcessRunner::new()
    }

    fn valid_plan() -> crate::policy::AndroidSandboxPlan {
        crate::policy::AndroidSandboxPlan {
            target: crate::policy::ExecTarget::SystemShell,
            argv: vec!["sh".into()],
            env: vec![],
            network: crate::policy::NetProfile::DenyNet,
            rlimits: vec![],
            seccomp_policy: None,
            cleanup: crate::policy::ProcessCleanup::KillProcessGroup,
        }
    }

    #[tokio::test]
    async fn rejects_bypass_audited_commands() {
        let sc = SandboxedCommand::__new_sandboxed(
            cmd(),
            SandboxedTag::BypassAuditedWithReason {
                reason: "bash_tool_call".into(),
            },
        );
        let err = runner().run(&sc).await.unwrap_err();
        assert!(matches!(err, ProcessError::MalformedSandboxPlan(ref m)
            if m.contains("bypass")));
    }

    #[tokio::test]
    async fn rejects_foreign_backend() {
        let sc = SandboxedCommand::__new_sandboxed(
            cmd(),
            SandboxedTag::Wrapped {
                backend: SandboxBackend::MacOsSandboxExec,
            },
        );
        let err = runner().run(&sc).await.unwrap_err();
        assert!(matches!(err, ProcessError::MalformedSandboxPlan(ref m)
            if m.contains("backend")));
    }

    #[tokio::test]
    async fn rejects_missing_or_foreign_plan() {
        let missing = SandboxedCommand::__new_sandboxed(
            cmd(),
            SandboxedTag::Wrapped {
                backend: SandboxBackend::AndroidMinijail,
            },
        );
        let err = runner().run(&missing).await.unwrap_err();
        assert!(matches!(err, ProcessError::MalformedSandboxPlan(ref m)
            if m.contains("missing")));

        let foreign = SandboxedCommand::__new_sandboxed_with_plan(
            cmd(),
            SandboxedTag::Wrapped {
                backend: SandboxBackend::AndroidMinijail,
            },
            BackendPlanHandle::new(String::from("not a plan")),
        );
        let err = runner().run(&foreign).await.unwrap_err();
        assert!(matches!(err, ProcessError::MalformedSandboxPlan(ref m)
            if m.contains("downcast")));
    }

    #[tokio::test]
    async fn valid_plan_is_unsupported_until_p2() {
        let sc = SandboxedCommand::__new_sandboxed_with_plan(
            cmd(),
            SandboxedTag::Wrapped {
                backend: SandboxBackend::AndroidMinijail,
            },
            BackendPlanHandle::new(valid_plan()),
        );
        let err = runner().run(&sc).await.unwrap_err();
        assert!(matches!(err, ProcessError::Unsupported));
        assert!(!runner().is_available());
    }

    #[tokio::test]
    async fn bypass_tag_with_valid_plan_attached_still_rejected() {
        let sc = SandboxedCommand::__new_sandboxed_with_plan(
            cmd(),
            SandboxedTag::BypassAuditedWithReason {
                reason: "bash_tool_call".into(),
            },
            BackendPlanHandle::new(valid_plan()),
        );
        let err = runner().run(&sc).await.unwrap_err();
        assert!(matches!(err, ProcessError::MalformedSandboxPlan(ref m) if m.contains("bypass")));
    }

    #[tokio::test]
    async fn foreign_backend_with_valid_plan_attached_still_rejected() {
        let sc = SandboxedCommand::__new_sandboxed_with_plan(
            cmd(),
            SandboxedTag::Wrapped {
                backend: SandboxBackend::MacOsSandboxExec,
            },
            BackendPlanHandle::new(valid_plan()),
        );
        let err = runner().run(&sc).await.unwrap_err();
        assert!(matches!(err, ProcessError::MalformedSandboxPlan(ref m) if m.contains("backend")));
    }
}
