//! `SandboxPolicy` → [`AndroidSandboxPlan`] mapping (spec r3 §Policy mapping).
//!
//! Pure functions, no I/O: validation/planning happens in `sandbox.rs`,
//! execution in `process.rs`. Fail-closed: every unenforceable request is
//! rejected with a named guarantee.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use traits::{NetworkPolicy, SandboxError, SandboxPolicy};

/// What the runner will ultimately `execve` (spec r3 §`AndroidSandboxPlan`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecTarget {
    /// `/system/bin/sh -c <command>` — the v1 interpreter; becomes the
    /// internal diagnostic fallback after P5.
    SystemShell,
    /// A packaged executable under `nativeLibraryDir` (git at P4, bundled
    /// mksh/toybox at P5). Identity-checked: canonical path + content hash.
    BundledHelper {
        /// Helper short name (`"git"`, `"mksh"`, …).
        name: String,
        /// Canonicalized absolute path under `nativeLibraryDir`.
        path: PathBuf,
        /// Hex SHA-256 of the binary, recorded in capabilities + receipts.
        hash: String,
    },
}

/// Network stance compiled into the jail (spec D10: Shell is `DenyNet` always;
/// only structured tools may build `AllowNet` plans).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetProfile {
    /// seccomp denies socket-family syscalls for the whole process tree.
    DenyNet,
    /// No network restriction (structured Git tool only, P4+).
    AllowNet,
}

/// One rlimit the jail applies before exec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rlimit {
    /// Which resource.
    pub resource: RlimitResource,
    /// Soft limit value.
    pub soft: u64,
    /// Hard limit value.
    pub hard: u64,
}

/// The subset of rlimits the spec maps (spec r3 §Policy mapping).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RlimitResource {
    /// `RLIMIT_CPU` (seconds).
    Cpu,
    /// `RLIMIT_AS` (bytes, best effort).
    As,
    /// `RLIMIT_NOFILE`.
    NoFile,
    /// `RLIMIT_CORE` (always 0).
    Core,
}

/// Reference to a compiled seccomp filter (name + content hash for receipts).
/// `None` in P1 — the net-deny filter is compiled in P2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeccompRef {
    /// Filter name (`"net-deny-v1"`).
    pub name: String,
    /// Hex SHA-256 of the compiled BPF program.
    pub hash: String,
}

/// How the runner tears the child down on timeout/cancel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessCleanup {
    /// `setsid()` at spawn; `kill(-pgid, SIGKILL)` on expiry.
    KillProcessGroup,
}

/// The prepared, in-process-only execution plan (never serialized — spec D7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AndroidSandboxPlan {
    /// Exec target.
    pub target: ExecTarget,
    /// Full argv (argv[0] included).
    pub argv: Vec<String>,
    /// Post-scrub environment (the ONLY env the child sees).
    pub env: Vec<(String, String)>,
    /// Network stance.
    pub network: NetProfile,
    /// Rlimits to apply.
    pub rlimits: Vec<Rlimit>,
    /// Compiled seccomp filter reference (P2+; `None` in P1).
    pub seccomp_policy: Option<SeccompRef>,
    /// Teardown strategy.
    pub cleanup: ProcessCleanup,
}

/// Map a [`SandboxPolicy`] onto an [`AndroidSandboxPlan`], failing closed with
/// a **named guarantee** for everything Android cannot enforce
/// (spec r3 §Policy mapping). Pure; argv is filled by the caller.
///
/// # Errors
/// `SandboxError::Unavailable` naming the unenforceable guarantee.
pub fn plan_from_policy(
    target: ExecTarget,
    policy: &SandboxPolicy,
    env: Vec<(String, String)>,
) -> Result<AndroidSandboxPlan, SandboxError> {
    let network = match (policy.network, &target) {
        (NetworkPolicy::LoopbackOnly, _) => {
            return Err(SandboxError::Unavailable(
                "loopback-only network policy is unenforceable on Android (seccomp cannot \
                 inspect sockaddr)"
                    .into(),
            ));
        }
        (NetworkPolicy::Allowed, ExecTarget::SystemShell) => {
            return Err(SandboxError::Unavailable(
                "networked shell is not supported; network grants exist only on structured \
                 tools (spec D10)"
                    .into(),
            ));
        }
        (NetworkPolicy::Allowed, ExecTarget::BundledHelper { .. }) => NetProfile::AllowNet,
        (NetworkPolicy::Disabled, _) => NetProfile::DenyNet,
    };

    if !policy.writable_paths.is_empty() || !policy.denied_paths.is_empty() {
        return Err(SandboxError::Unavailable(
            "filesystem confinement requires Landlock, which shipping Android kernels do not \
             enable; the default mobile policy must request none"
                .into(),
        ));
    }

    if !policy.allow_subprocess {
        return Err(SandboxError::Unavailable(
            "subprocess denial is not enforceable for shell/git targets on Android (both \
             require children)"
                .into(),
        ));
    }

    let mut rlimits = Vec::new();
    if let Some(cpu) = policy.limits.max_cpu_seconds {
        rlimits.push(Rlimit {
            resource: RlimitResource::Cpu,
            soft: u64::from(cpu),
            hard: u64::from(cpu),
        });
    }
    if let Some(mem_mb) = policy.limits.max_memory_mb {
        let bytes = u64::from(mem_mb) * 1024 * 1024;
        rlimits.push(Rlimit {
            resource: RlimitResource::As,
            soft: bytes,
            hard: bytes,
        });
    }
    if let Some(nofile) = policy.limits.max_open_files {
        rlimits.push(Rlimit {
            resource: RlimitResource::NoFile,
            soft: u64::from(nofile),
            hard: u64::from(nofile),
        });
    }
    // limits.max_processes is intentionally NOT mapped: RLIMIT_NPROC is
    // UID-scoped on Android and would count every process of the whole app.
    rlimits.push(Rlimit {
        resource: RlimitResource::Core,
        soft: 0,
        hard: 0,
    });

    Ok(AndroidSandboxPlan {
        target,
        argv: Vec::new(),
        env,
        network,
        rlimits,
        // The compiled net-deny BPF lands in P2; the plan records the stance
        // via `network` either way.
        seccomp_policy: None,
        cleanup: ProcessCleanup::KillProcessGroup,
    })
}

/// Keys that are never forwarded from the caller, even if explicitly set
/// (spec r3 §Environment: security denylist — injection + credential vectors).
const ENV_DENYLIST: &[&str] = &[
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "DYLD_INSERT_LIBRARIES",
    "DYLD_LIBRARY_PATH",
    "ANTHROPIC_API_KEY",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "GOOGLE_APPLICATION_CREDENTIALS",
];

/// Build the child environment: the inherited environment is DISCARDED
/// entirely and rebuilt from the spec's allowlist table (spec r3 §Environment);
/// `caller_env` (the tool's explicit `cmd.env`) overlays last — explicit wins,
/// except keys in the security denylist which are always stripped.
#[must_use]
pub fn build_shell_env<S: std::hash::BuildHasher>(
    workspace_root: &Path,
    cache_dir: &Path,
    bundled_helper_dir: Option<&Path>,
    caller_env: &HashMap<String, String, S>,
) -> Vec<(String, String)> {
    let path_value = match bundled_helper_dir {
        Some(dir) => format!("{}:/system/bin", dir.display()),
        None => "/system/bin".to_string(),
    };
    let mut env: Vec<(String, String)> = vec![
        ("HOME".into(), workspace_root.display().to_string()),
        ("TMPDIR".into(), cache_dir.display().to_string()),
        ("PATH".into(), path_value),
        ("LANG".into(), "C.UTF-8".into()),
        ("TERM".into(), "dumb".into()),
        ("ANDROID_ROOT".into(), "/system".into()),
        ("ANDROID_DATA".into(), "/data".into()),
    ];
    for (k, v) in caller_env {
        if ENV_DENYLIST.contains(&k.as_str()) {
            continue;
        }
        if let Some(slot) = env.iter_mut().find(|(name, _)| name == k) {
            slot.1.clone_from(v);
        } else {
            env.push((k.clone(), v.clone()));
        }
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use traits::{NetworkPolicy, ResourceLimits, SandboxError, SandboxPolicy};

    #[test]
    fn plan_types_construct() {
        let plan = AndroidSandboxPlan {
            target: ExecTarget::SystemShell,
            argv: vec!["sh".into(), "-c".into(), "true".into()],
            env: vec![("HOME".into(), "/data/x".into())],
            network: NetProfile::DenyNet,
            rlimits: vec![Rlimit {
                resource: RlimitResource::Core,
                soft: 0,
                hard: 0,
            }],
            seccomp_policy: None,
            cleanup: ProcessCleanup::KillProcessGroup,
        };
        assert!(matches!(plan.target, ExecTarget::SystemShell));
        assert!(matches!(plan.network, NetProfile::DenyNet));
    }

    fn base_policy() -> SandboxPolicy {
        SandboxPolicy {
            network: NetworkPolicy::Disabled,
            writable_paths: vec![],
            denied_paths: vec![],
            allow_subprocess: true,
            limits: ResourceLimits::default(),
        }
    }

    fn assert_unavailable_naming(result: Result<AndroidSandboxPlan, SandboxError>, needle: &str) {
        match result {
            Err(SandboxError::Unavailable(msg)) => {
                assert!(msg.contains(needle), "message {msg:?} must name {needle:?}");
            }
            other => panic!("expected Unavailable naming {needle:?}, got {other:?}"),
        }
    }

    #[test]
    fn loopback_only_fails_closed_with_named_guarantee() {
        let mut p = base_policy();
        p.network = NetworkPolicy::LoopbackOnly;
        assert_unavailable_naming(
            plan_from_policy(ExecTarget::SystemShell, &p, vec![]),
            "loopback-only",
        );
    }

    #[test]
    fn fs_confinement_fails_closed() {
        let mut p = base_policy();
        p.writable_paths = vec![PathBuf::from("/data/x")];
        assert_unavailable_naming(
            plan_from_policy(ExecTarget::SystemShell, &p, vec![]),
            "Landlock",
        );
        let mut p = base_policy();
        p.denied_paths = vec![PathBuf::from("/data/y")];
        assert_unavailable_naming(
            plan_from_policy(ExecTarget::SystemShell, &p, vec![]),
            "Landlock",
        );
    }

    #[test]
    fn subprocess_denial_fails_closed() {
        let mut p = base_policy();
        p.allow_subprocess = false;
        assert_unavailable_naming(
            plan_from_policy(ExecTarget::SystemShell, &p, vec![]),
            "subprocess",
        );
    }

    #[test]
    fn networked_system_shell_fails_closed() {
        let mut p = base_policy();
        p.network = NetworkPolicy::Allowed;
        assert_unavailable_naming(
            plan_from_policy(ExecTarget::SystemShell, &p, vec![]),
            "networked shell",
        );
    }

    #[test]
    fn networked_bundled_helper_is_allowed() {
        let mut p = base_policy();
        p.network = NetworkPolicy::Allowed;
        let plan = plan_from_policy(
            ExecTarget::BundledHelper {
                name: "git".into(),
                path: PathBuf::from("/data/app/x/lib/arm64/libgit.so"),
                hash: "abc".into(),
            },
            &p,
            vec![],
        )
        .expect("bundled helper may request network");
        assert_eq!(plan.network, NetProfile::AllowNet);
    }

    #[test]
    fn limits_map_to_rlimits_with_core_always_zero() {
        let mut p = base_policy();
        p.limits = ResourceLimits {
            max_cpu_seconds: Some(30),
            max_memory_mb: Some(512),
            max_processes: Some(8), // intentionally NOT mapped (UID-scoped NPROC)
            max_open_files: Some(256),
        };
        let plan = plan_from_policy(ExecTarget::SystemShell, &p, vec![]).expect("plan");
        assert!(plan.rlimits.contains(&Rlimit {
            resource: RlimitResource::Cpu,
            soft: 30,
            hard: 30
        }));
        assert!(plan.rlimits.contains(&Rlimit {
            resource: RlimitResource::As,
            soft: 512 * 1024 * 1024,
            hard: 512 * 1024 * 1024
        }));
        assert!(plan.rlimits.contains(&Rlimit {
            resource: RlimitResource::NoFile,
            soft: 256,
            hard: 256
        }));
        assert!(plan.rlimits.contains(&Rlimit {
            resource: RlimitResource::Core,
            soft: 0,
            hard: 0
        }));
        // max_processes intentionally unmapped:
        assert_eq!(
            plan.rlimits.len(),
            4,
            "NPROC must not be mapped in v1 (UID-scoped)"
        );
        assert_eq!(plan.network, NetProfile::DenyNet);
        assert!(plan.seccomp_policy.is_none(), "filter compiled in P2");
        assert_eq!(plan.cleanup, ProcessCleanup::KillProcessGroup);
    }

    #[test]
    fn env_is_scrubbed_and_rebuilt_from_allowlist() {
        let mut inherited = HashMap::new();
        inherited.insert("LD_PRELOAD".to_string(), "/evil.so".to_string());
        inherited.insert("ANTHROPIC_API_KEY".to_string(), "sk-secret".to_string());
        let env = build_shell_env(
            std::path::Path::new("/data/user/0/app/files/workspace"),
            std::path::Path::new("/data/user/0/app/cache"),
            None,
            &inherited,
        );
        let map: HashMap<_, _> = env.iter().cloned().collect();
        assert_eq!(map.get("HOME").map(String::as_str), Some("/data/user/0/app/files/workspace"));
        assert_eq!(map.get("TMPDIR").map(String::as_str), Some("/data/user/0/app/cache"));
        assert_eq!(map.get("PATH").map(String::as_str), Some("/system/bin"));
        assert_eq!(map.get("LANG").map(String::as_str), Some("C.UTF-8"));
        assert_eq!(map.get("TERM").map(String::as_str), Some("dumb"));
        assert_eq!(map.get("ANDROID_ROOT").map(String::as_str), Some("/system"));
        assert_eq!(map.get("ANDROID_DATA").map(String::as_str), Some("/data"));
        assert!(!map.contains_key("LD_PRELOAD"), "inherited env must be scrubbed");
        assert!(!map.contains_key("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn bundled_dir_shadows_system_in_path_and_caller_env_overlays() {
        let mut caller = HashMap::new();
        caller.insert("GIT_TRACE".to_string(), "1".to_string());
        caller.insert("TERM".to_string(), "xterm".to_string()); // explicit wins
        let env = build_shell_env(
            std::path::Path::new("/w"),
            std::path::Path::new("/c"),
            Some(std::path::Path::new("/data/app/x/lib/arm64")),
            &caller,
        );
        let map: HashMap<_, _> = env.iter().cloned().collect();
        assert_eq!(
            map.get("PATH").map(String::as_str),
            Some("/data/app/x/lib/arm64:/system/bin")
        );
        assert_eq!(map.get("GIT_TRACE").map(String::as_str), Some("1"));
        assert_eq!(map.get("TERM").map(String::as_str), Some("xterm"));
    }
}
