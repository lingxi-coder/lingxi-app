//! One-shot capability probe + session cache (spec r3 §Capability probing).
//!
//! Eager (D8): `engine_mobile::build_mobile_engine` runs the probe via
//! `block_on` BEFORE the synchronous tool registry is assembled; everything
//! downstream (`prepare()`, registration gates) reads the cache only.

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
                fs_readwrite_paths: self.landlock_abi.is_some(),
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
        // P0a Task 16 replaces this body with the minijail smoke call.
        AndroidSandboxCapabilities::unavailable("on-device probe not yet implemented (P0a/P2)")
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
}
