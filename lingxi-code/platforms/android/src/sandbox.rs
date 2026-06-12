//! [`traits::Sandbox`] impl for Android — validation + plan construction only;
//! never spawns (spec r3 §`AndroidMinijailSandbox`).

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use traits::{
    BackendPlanHandle, ProcessCommand, Sandbox, SandboxBackend, SandboxCapability, SandboxError,
    SandboxPolicy, SandboxedCommand, SandboxedTag,
};

use crate::capabilities::CapabilityCache;
use crate::config::AndroidShellConfig;
use crate::policy::{build_shell_env, plan_from_policy, ExecTarget};

/// Android sandbox: maps policies onto Minijail plans, fail-closed.
pub struct AndroidMinijailSandbox {
    cfg: AndroidShellConfig,
    caps: Arc<CapabilityCache>,
}

impl AndroidMinijailSandbox {
    /// Construct from host config + the (eagerly populated) capability cache.
    #[must_use]
    pub fn new(cfg: AndroidShellConfig, caps: Arc<CapabilityCache>) -> Self {
        Self { cfg, caps }
    }

    /// Canonicalize `cwd` (default: workspace root) and require containment
    /// inside the canonicalized workspace root — symlink escapes are refused.
    fn resolve_cwd(&self, cwd: Option<&PathBuf>) -> Result<PathBuf, SandboxError> {
        let root = self
            .cfg
            .shell_workspace_root
            .canonicalize()
            .map_err(|e| SandboxError::PathCanonicalize(format!("workspace root: {e}")))?;
        let requested = cwd.cloned().unwrap_or_else(|| root.clone());
        let canon = requested
            .canonicalize()
            .map_err(|e| SandboxError::PathCanonicalize(format!("{}: {e}", requested.display())))?;
        if !canon.starts_with(&root) {
            return Err(SandboxError::SymlinkEscape(canon.display().to_string()));
        }
        Ok(canon)
    }
}

#[async_trait]
impl Sandbox for AndroidMinijailSandbox {
    fn is_available(&self) -> bool {
        self.caps.get().available()
    }

    fn backend(&self) -> SandboxBackend {
        SandboxBackend::AndroidMinijail
    }

    fn prepare(
        &self,
        cmd: ProcessCommand,
        policy: &SandboxPolicy,
    ) -> Result<SandboxedCommand, SandboxError> {
        let caps = self.caps.get();
        if !caps.available() {
            return Err(SandboxError::Unavailable(
                caps.reason
                    .unwrap_or_else(|| "android sandbox unavailable".into()),
            ));
        }

        // P2 NOTE: when the net-deny seccomp filter lands, DenyNet plan
        // admission must additionally gate on caps.seccomp_filter +
        // caps.net_deny_verified — available() alone is the coarse
        // registration gate, not the per-plan requirement check.

        let resolved_cwd = self.resolve_cwd(cmd.cwd.as_ref())?;

        let env = build_shell_env(
            &resolved_cwd,
            &self.cfg.app_cache_root,
            None, // bundled helper dir joins the PATH in P4
            &cmd.env,
        );

        let mut plan = plan_from_policy(ExecTarget::SystemShell, policy, env)?;
        // argv[0] convention: "sh"; the runner execs /system/bin/sh.
        plan.argv = std::iter::once("sh".to_string())
            .chain(cmd.args.iter().cloned())
            .collect();

        let inner = ProcessCommand {
            cwd: Some(resolved_cwd),
            ..cmd
        };
        Ok(SandboxedCommand::__new_sandboxed_with_plan(
            inner,
            SandboxedTag::Wrapped {
                backend: SandboxBackend::AndroidMinijail,
            },
            BackendPlanHandle::new(plan),
        ))
    }

    fn bypass_with_audit(&self, cmd: ProcessCommand, reason: &str) -> SandboxedCommand {
        // Trait contract: always mints. The Android RUNNER rejects this tag
        // (spec security invariant #1) — the audit trail still records intent.
        SandboxedCommand::__new_sandboxed(
            cmd,
            SandboxedTag::BypassAuditedWithReason {
                reason: reason.to_string(),
            },
        )
    }

    async fn probe_capability(&self) -> SandboxCapability {
        self.caps.get().to_sandbox_capability()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::AndroidSandboxCapabilities;
    use crate::policy::{AndroidSandboxPlan, NetProfile}; // not in the impl's imports — tests need it explicitly
    use std::collections::HashMap;
    use traits::{NetworkPolicy, ProcessCommand, ResourceLimits, Sandbox, SandboxPolicy};

    fn ready_caps() -> AndroidSandboxCapabilities {
        AndroidSandboxCapabilities {
            probed: true,
            minijail_smoke: true,
            no_new_privs: true,
            ..AndroidSandboxCapabilities::default()
        }
    }

    fn shell_cfg(ws: &std::path::Path) -> AndroidShellConfig {
        AndroidShellConfig {
            native_library_dir: ws.join("native-lib"),
            shell_workspace_root: ws.to_path_buf(),
            app_cache_root: ws.join("cache"),
            package_name: "com.example".into(),
            package_version_code: 1,
            app_writable_roots: vec![ws.to_path_buf()],
            enable_shell: true,
            secrets_in_keystore: true,
            shell_data_exposure_accepted: false,
        }
    }

    fn sandbox_with(
        ws: &std::path::Path,
        caps: AndroidSandboxCapabilities,
    ) -> AndroidMinijailSandbox {
        let cache = std::sync::Arc::new(CapabilityCache::new());
        cache.set(caps);
        AndroidMinijailSandbox::new(shell_cfg(ws), cache)
    }

    fn cmd(cwd: Option<std::path::PathBuf>) -> ProcessCommand {
        ProcessCommand {
            command: "/system/bin/sh".into(),
            args: vec!["-c".into(), "true".into()],
            cwd,
            env: HashMap::new(),
            timeout: None,
            stdin: None,
        }
    }

    fn deny_net_policy() -> SandboxPolicy {
        SandboxPolicy {
            network: NetworkPolicy::Disabled,
            writable_paths: vec![],
            denied_paths: vec![],
            allow_subprocess: true,
            limits: ResourceLimits::default(),
        }
    }

    #[test]
    fn prepare_attaches_android_plan_and_tag() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sb = sandbox_with(tmp.path(), ready_caps());
        let sc = sb.prepare(cmd(None), &deny_net_policy()).expect("prepare");
        assert!(matches!(
            sc.tag(),
            traits::SandboxedTag::Wrapped {
                backend: traits::SandboxBackend::AndroidMinijail
            }
        ));
        let plan = sc
            .backend_plan()
            .expect("plan attached")
            .downcast::<AndroidSandboxPlan>()
            .expect("android plan type");
        assert!(matches!(plan.target, ExecTarget::SystemShell));
        assert_eq!(plan.network, NetProfile::DenyNet);
        assert_eq!(
            plan.argv,
            vec!["sh".to_string(), "-c".to_string(), "true".to_string()]
        );
        let env: std::collections::HashMap<_, _> = plan.env.iter().cloned().collect();
        assert_eq!(
            env.get("HOME").map(String::as_str),
            Some(tmp.path().canonicalize().unwrap().to_str().unwrap())
        );
    }

    #[test]
    fn prepare_fails_closed_when_caps_unavailable() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sb = sandbox_with(
            tmp.path(),
            AndroidSandboxCapabilities::unavailable("no device probe"),
        );
        let err = sb.prepare(cmd(None), &deny_net_policy()).unwrap_err();
        assert!(matches!(err, traits::SandboxError::Unavailable(_)));
    }

    #[test]
    fn prepare_rejects_cwd_outside_workspace() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("outside");
        let sb = sandbox_with(tmp.path(), ready_caps());
        let err = sb
            .prepare(cmd(Some(outside.path().to_path_buf())), &deny_net_policy())
            .unwrap_err();
        assert!(matches!(err, traits::SandboxError::SymlinkEscape(_)));
    }

    #[test]
    fn prepare_rejects_symlink_escape() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("outside");
        let link = tmp.path().join("sneaky");
        std::os::unix::fs::symlink(outside.path(), &link).expect("symlink");
        let sb = sandbox_with(tmp.path(), ready_caps());
        let err = sb.prepare(cmd(Some(link)), &deny_net_policy()).unwrap_err();
        assert!(matches!(err, traits::SandboxError::SymlinkEscape(_)));
    }

    #[test]
    fn prepare_propagates_policy_fail_closed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sb = sandbox_with(tmp.path(), ready_caps());
        let mut policy = deny_net_policy();
        policy.network = NetworkPolicy::Allowed; // SystemShell + Allowed = refused
        assert!(sb.prepare(cmd(None), &policy).is_err());
    }

    #[test]
    fn bypass_with_audit_still_mints_bypass_tag() {
        // Trait contract: bypass always returns a SandboxedCommand. The
        // ANDROID RUNNER is what rejects it (Task 10) — not prepare.
        let tmp = tempfile::tempdir().expect("tempdir");
        let sb = sandbox_with(tmp.path(), ready_caps());
        let sc = sb.bypass_with_audit(cmd(None), "test-reason");
        assert!(matches!(
            sc.tag(),
            traits::SandboxedTag::BypassAuditedWithReason { .. }
        ));
        assert!(sc.backend_plan().is_none());
    }

    #[tokio::test]
    async fn probe_capability_reads_cache() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sb = sandbox_with(tmp.path(), ready_caps());
        let cap = sb.probe_capability().await;
        assert!(cap.available);
        assert!(cap.features.no_new_privileges);
    }
}
