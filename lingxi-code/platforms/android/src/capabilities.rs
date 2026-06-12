//! One-shot capability probe + session cache (spec r3 §Capability probing).
//!
//! The eager probe seam is wired in P2. Two options under consideration: either
//! the probe runs inside `AndroidMinijailSandbox::probe_capability()`, which can
//! call `CapabilityCache::set()` on its own field and is reachable through the
//! `Sandbox` trait object, or `AndroidPlatform` grows a cache accessor the
//! engine can `block_on` before assembling the tool registry. The P2 plan decides.

use std::sync::OnceLock;

use traits::{SandboxCapability, SandboxFeatures};

/// Probed-by-real-behavior capability matrix (never inferred from API level).
#[derive(Debug, Clone, Default)]
#[allow(clippy::struct_excessive_bools)] // mirrors the spec's probe list verbatim
pub struct AndroidSandboxCapabilities {
    /// The probe actually ran on this device (vs. conservative default).
    pub probed: bool,
    /// libminijail linked and a jailed `sh -c true` fork+exec smoke passed.
    pub minijail_smoke: bool,
    /// `no_new_privs` could be set in a disposable child.
    pub no_new_privs: bool,
    /// A harmless seccomp filter installed in a disposable child.
    pub seccomp_filter: bool,
    /// seccomp TSYNC available.
    pub seccomp_tsync: bool,
    /// Probe child observed `socket()` ⇒ `EPERM` under the net-deny filter.
    pub net_deny_verified: bool,
    /// `kill(-pgid)` tears down a probe process group.
    pub pgid_kill: bool,
    /// Landlock ABI version when present (expected `None` on devices).
    pub landlock_abi: Option<u32>,
    /// `/system/bin/sh` present; `KSH_VERSION` when readable.
    pub system_sh_version: Option<String>,
    /// Probed toybox applet inventory (feeds the Shell tool prompt).
    pub toybox_applets: Vec<String>,
    /// Why the sandbox is unavailable, when it is.
    pub reason: Option<String>,
}

impl AndroidSandboxCapabilities {
    /// Conservative "cannot run" capabilities with a reason.
    #[must_use]
    pub fn unavailable(reason: &str) -> Self {
        Self {
            reason: Some(reason.to_string()),
            ..Self::default()
        }
    }

    /// Whether `prepare()` may admit commands at all.
    ///
    /// This is the coarse registration gate (spec gate #2); per-plan
    /// requirements (e.g. the net-deny seccomp filter for deny-net plans)
    /// are checked in `prepare()` itself, not here.
    #[must_use]
    pub fn available(&self) -> bool {
        self.probed && self.minijail_smoke && self.no_new_privs
    }

    /// Conservative cross-platform feature report (spec: never overstate —
    /// `fs_readonly`/`fs_readwrite_paths` stay false without Landlock).
    #[must_use]
    pub fn to_sandbox_capability(&self) -> SandboxCapability {
        SandboxCapability {
            available: self.available(),
            reason: self.reason.clone(),
            features: SandboxFeatures {
                network_isolation: false, // seccomp deny ≠ namespace isolation
                fs_readonly: false,
                // Landlock ABI is recorded, not relied on: `plan_from_policy`
                // unconditionally rejects FS confinement (no enforcement path
                // exists). Flip only when one does — never overstate.
                fs_readwrite_paths: false,
                process_limit: self.seccomp_filter,
                no_new_privileges: self.no_new_privs,
            },
        }
    }
}

/// Session-lifetime cache. Set exactly once by the eager probe; read by
/// `prepare()` and the registration gates.
#[derive(Debug, Default)]
pub struct CapabilityCache(OnceLock<AndroidSandboxCapabilities>);

impl CapabilityCache {
    /// Construct an empty (un-probed) cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Store the probe result. Later calls are ignored (first write wins).
    pub fn set(&self, caps: AndroidSandboxCapabilities) {
        let _ = self.0.set(caps);
    }

    /// Read the cached result; un-probed reads as conservative-unavailable.
    pub fn get(&self) -> AndroidSandboxCapabilities {
        self.0.get().cloned().unwrap_or_else(|| {
            AndroidSandboxCapabilities::unavailable("capability probe has not run")
        })
    }
}

/// Run the capability probe.
///
/// Host (non-Android) builds: the sandbox is structurally absent — return
/// the conservative result so host tests exercise the gates.
/// Android builds: P0a Task 16 wires the minijail smoke; the remaining probe
/// items land with the P2 runner.
// The async signature is the trait-facing seam the engine `block_on`s (matching
// the pattern used in `platforms/posix/src/mcp.rs` `spawn_stdio_with_handles`).
// The host-build body has no `.await` — suppress the lint rather than drop the
// async seam.
#[allow(clippy::unused_async)]
pub async fn probe_android_capabilities() -> AndroidSandboxCapabilities {
    #[cfg(not(target_os = "android"))]
    {
        AndroidSandboxCapabilities::unavailable("android sandbox requires an Android device")
    }
    #[cfg(target_os = "android")]
    {
        let smoke = platform_android_minijail::minijail_smoke();
        AndroidSandboxCapabilities {
            probed: true,
            minijail_smoke: smoke.ok,
            no_new_privs: smoke.no_new_privs,
            // The remaining probe items (seccomp install, TSYNC, net-deny
            // socket()==EPERM, pgid kill, landlock ABI, sh version, toybox
            // inventory) land with the P2 runner plan.
            reason: smoke.reason,
            ..AndroidSandboxCapabilities::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unprobed_cache_reads_unavailable() {
        let cache = CapabilityCache::new();
        let caps = cache.get();
        assert!(!caps.available());
        assert!(caps.reason.as_deref().unwrap_or("").contains("not run"));
    }

    #[test]
    fn first_set_wins_and_available_needs_smoke_plus_nnp() {
        let cache = CapabilityCache::new();
        cache.set(AndroidSandboxCapabilities {
            probed: true,
            minijail_smoke: true,
            no_new_privs: true,
            ..AndroidSandboxCapabilities::default()
        });
        cache.set(AndroidSandboxCapabilities::unavailable("late write"));
        assert!(cache.get().available(), "first write must win");
    }

    #[tokio::test]
    async fn host_probe_is_conservative() {
        let caps = probe_android_capabilities().await;
        assert!(!caps.available());
        let cap = caps.to_sandbox_capability();
        assert!(!cap.available);
        assert!(!cap.features.network_isolation);
        assert!(!cap.features.fs_readonly);
    }

    #[test]
    fn available_requires_every_gate_conjunct() {
        let full = AndroidSandboxCapabilities {
            probed: true,
            minijail_smoke: true,
            no_new_privs: true,
            ..AndroidSandboxCapabilities::default()
        };
        assert!(full.available());
        for missing in ["probed", "smoke", "nnp"] {
            let mut c = full.clone();
            match missing {
                "probed" => c.probed = false,
                "smoke" => c.minijail_smoke = false,
                _ => c.no_new_privs = false,
            }
            assert!(!c.available(), "gate must fail without {missing}");
        }
    }

    #[test]
    fn landlock_abi_presence_does_not_overstate_fs_features() {
        let caps = AndroidSandboxCapabilities {
            probed: true,
            minijail_smoke: true,
            no_new_privs: true,
            landlock_abi: Some(4),
            ..AndroidSandboxCapabilities::default()
        };
        let cap = caps.to_sandbox_capability();
        // No FS enforcement path exists (plan_from_policy rejects FS
        // confinement unconditionally) — the flag must stay false.
        assert!(!cap.features.fs_readwrite_paths);
        assert!(!cap.features.fs_readonly);
    }
}
