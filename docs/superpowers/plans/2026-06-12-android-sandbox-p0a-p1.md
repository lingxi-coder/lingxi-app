# Android Sandbox P0a+P1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the Android sandbox foundations from the r3 spec — trait extensions, `AndroidMinijailSandbox::prepare()` with fail-closed policy mapping, the runner's security invariants (execution still disabled), config/FFI plumbing, and the P0a libminijail NDK build + on-device smoke gate.

**Architecture:** Spec `docs/superpowers/specs/2026-06-12-android-sandbox-shell-design.md` (r3). This plan covers **P0a + P1 only**; P2 (runner execution), P3 (Shell tool), P4 (Git tool), P5 (bundled interpreter) get their own follow-on plans. P1 is pure host-testable Rust and ordered first; P0a (NDK build + device smoke) is second and is the **global gate for P2+** — no execution work may start until the P0a smoke passes on a device, but P1 host work is gate-independent.

**Tech Stack:** Rust workspace at `lingxi-code/` (run all cargo commands from there). `traits` crate (Sandbox/ProcessRunner seam), `platform-android`, `android-aar` (UniFFI 0.28.3), vendored `third_party/minijail` (incl. `rust/minijail` safe wrapper + `rust/minijail-sys`), Android NDK + `cargo-ndk`, Gradle 9.3 instrumentation harness at `apps/android-aar/kotlin/`.

**Spec invariants this plan must not violate:**
- `Sandbox::prepare()` is the only admission path; the Android runner rejects `BypassAuditedWithReason`, wrong backends, and missing plans.
- Fail closed with named guarantees at `prepare()`; no silent downgrade.
- v1 has zero exec-packaging risk: libminijail enters as a **library** (W^X-exempt); nothing in this plan installs an executable.
- `platform-android` keeps `#![forbid(unsafe_code)]` — all unsafe FFI lives in the new `platform-android-minijail` crate.

---

## File structure

```text
lingxi-code/
├── traits/src/process.rs                 MODIFY: +3 ProcessError variants
├── traits/src/sandbox.rs                 MODIFY: +AndroidMinijail variant, +BackendPlanHandle,
│                                                 +plan field, +__new_sandboxed_with_plan, +backend_plan()
├── platforms/android/src/lib.rs          MODIFY: module decls, shell wiring in AndroidPlatform::new
├── platforms/android/src/config.rs       CREATE: AndroidShellConfig
├── platforms/android/src/policy.rs       CREATE: ExecTarget/NetProfile/Rlimit/…/AndroidSandboxPlan,
│                                                 plan_from_policy, build_shell_env
├── platforms/android/src/capabilities.rs CREATE: AndroidSandboxCapabilities + CapabilityCache + probe
├── platforms/android/src/sandbox.rs      CREATE: AndroidMinijailSandbox
├── platforms/android/src/process.rs      CREATE: AndroidMinijailProcessRunner (invariants only)
├── platforms/android-minijail/           CREATE (P0a): safe smoke wrapper over minijail crate (unsafe allowed)
├── apps/android-aar/src/lib.rs           MODIFY: AndroidShellConfigFfi record, shell param,
│                                                 android_sandbox_smoke() export
├── apps/android-aar/kotlin/src/androidTest/…/SandboxSmokeTest.kt   CREATE (P0a)
└── Cargo.toml (workspace)                MODIFY: add platforms/android-minijail member
third_party/libcap/                       CREATE (P0a): vendored libcap sources
```

Responsibilities: `policy.rs` = pure policy→plan mapping (no I/O); `sandbox.rs` = validation + plan attachment; `process.rs` = runner invariants; `capabilities.rs` = probe cache; `config.rs` = host-supplied inputs; `android-minijail` = the only crate touching FFI.

---

# Part 1 — P1: traits + prepare skeleton (host-only, no device)

### Task 1: ProcessError structured variants

**Files:**
- Modify: `traits/src/process.rs:75-86`

- [ ] **Step 1: Write the failing test** — append at the end of `traits/src/process.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::ProcessError;

    #[test]
    fn structured_variants_name_the_guarantee() {
        assert_eq!(
            ProcessError::PolicyUnsupported("networked shell is not supported".into())
                .to_string(),
            "policy unsupported: networked shell is not supported"
        );
        assert_eq!(
            ProcessError::MalformedSandboxPlan("missing android plan".into()).to_string(),
            "malformed sandbox plan: missing android plan"
        );
        assert_eq!(
            ProcessError::SandboxEnforcementFailed("seccomp load failed".into()).to_string(),
            "sandbox enforcement failed: seccomp load failed"
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p traits structured_variants -- --nocapture`
Expected: FAIL — `no variant or associated item named 'PolicyUnsupported'`

- [ ] **Step 3: Add the variants** — in `traits/src/process.rs`, replace the `ProcessError` enum body:

```rust
/// Failure modes shared by every [`ProcessRunner`] method.
#[derive(Debug, Clone, Error)]
pub enum ProcessError {
    /// Platform does not implement process execution.
    #[error("unsupported on this platform")]
    Unsupported,
    /// A requested policy guarantee cannot be enforced on this platform.
    /// The payload names the guarantee (spec: "errors must name the
    /// unenforceable guarantee").
    #[error("policy unsupported: {0}")]
    PolicyUnsupported(String),
    /// The runner received a [`SandboxedCommand`] whose backend plan is
    /// missing, malformed, or minted for a different backend.
    #[error("malformed sandbox plan: {0}")]
    MalformedSandboxPlan(String),
    /// Jail setup failed at runtime (after prepare admitted the command).
    #[error("sandbox enforcement failed: {0}")]
    SandboxEnforcementFailed(String),
    /// Underlying I/O failure.
    #[error("io: {0}")]
    Io(String),
    /// Timeout fired before the command completed.
    #[error("timeout")]
    Timeout,
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p traits structured_variants`
Expected: PASS

- [ ] **Step 5: Verify no exhaustive matches broke** (known `ProcessError` users: `hooks/src/executor.rs`, `platforms/{posix,posix-minimal,windows}`, `tasks/src/handlers/local_bash.rs`, `test-harness/src/contracts/process.rs`, `apps/engine-desktop`)

Run: `cargo check --workspace`
Expected: clean (all existing uses are constructions / non-exhaustive matches)

- [ ] **Step 6: Commit**

```bash
git add traits/src/process.rs
git commit -m "feat(traits): structured ProcessError variants for android policy diagnostics"
```

---

### Task 2: SandboxBackend::AndroidMinijail

**Files:**
- Modify: `traits/src/sandbox.rs:54-65`

- [ ] **Step 1: Write the failing test** — inside the existing `#[cfg(test)] mod tests` at the bottom of `traits/src/sandbox.rs` add:

```rust
    #[test]
    fn android_minijail_backend_serde_roundtrip() {
        let b = SandboxBackend::AndroidMinijail;
        let json = serde_json::to_string(&b).expect("serialize");
        assert_eq!(json, "\"AndroidMinijail\"");
        let back: SandboxBackend = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, SandboxBackend::AndroidMinijail);
    }
```

(If `serde_json` is not yet a dev-dependency of `traits`, add to `traits/Cargo.toml` `[dev-dependencies]`: `serde_json = { workspace = true }`.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p traits android_minijail_backend`
Expected: FAIL — `no variant named 'AndroidMinijail'`

- [ ] **Step 3: Add the variant** — in the `SandboxBackend` enum after `WindowsJobObject`:

```rust
    /// Android in-engine Minijail (no_new_privs / rlimits / seccomp via
    /// libminijail linked into the engine .so). Spec r3 D6.
    AndroidMinijail,
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p traits && cargo check --workspace`
Expected: PASS / clean

- [ ] **Step 5: Commit**

```bash
git add traits/src/sandbox.rs traits/Cargo.toml
git commit -m "feat(traits): SandboxBackend::AndroidMinijail variant"
```

---

### Task 3: BackendPlanHandle + plan carriage on SandboxedCommand (spec D7)

**Files:**
- Modify: `traits/src/sandbox.rs:179-232`

- [ ] **Step 1: Write the failing tests** — in `traits/src/sandbox.rs` tests module:

```rust
    #[derive(Debug, PartialEq)]
    struct FakePlan {
        marker: u32,
    }

    fn cmd_for_plan_tests() -> ProcessCommand {
        ProcessCommand {
            command: "echo".into(),
            args: vec![],
            cwd: None,
            env: std::collections::HashMap::new(),
            timeout: None,
            stdin: None,
        }
    }

    #[test]
    fn new_sandboxed_has_no_plan() {
        let sc = SandboxedCommand::__new_sandboxed(
            cmd_for_plan_tests(),
            SandboxedTag::Wrapped {
                backend: SandboxBackend::None,
            },
        );
        assert!(sc.backend_plan().is_none());
    }

    #[test]
    fn with_plan_roundtrips_through_downcast() {
        let sc = SandboxedCommand::__new_sandboxed_with_plan(
            cmd_for_plan_tests(),
            SandboxedTag::Wrapped {
                backend: SandboxBackend::AndroidMinijail,
            },
            BackendPlanHandle::new(FakePlan { marker: 7 }),
        );
        let plan = sc
            .backend_plan()
            .expect("plan attached")
            .downcast::<FakePlan>()
            .expect("downcast to FakePlan");
        assert_eq!(plan.marker, 7);
        // Wrong type downcasts to None, not a panic.
        assert!(sc.backend_plan().unwrap().downcast::<String>().is_none());
    }

    #[test]
    fn plan_handle_debug_is_opaque_and_clone_shares() {
        let h = BackendPlanHandle::new(FakePlan { marker: 1 });
        assert_eq!(format!("{h:?}"), "BackendPlanHandle(..)");
        let sc = SandboxedCommand::__new_sandboxed_with_plan(
            cmd_for_plan_tests(),
            SandboxedTag::Wrapped {
                backend: SandboxBackend::AndroidMinijail,
            },
            h,
        );
        let cloned = sc.clone();
        assert!(cloned.backend_plan().unwrap().downcast::<FakePlan>().is_some());
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p traits plan`
Expected: FAIL — `BackendPlanHandle` not found

- [ ] **Step 3: Implement** — in `traits/src/sandbox.rs`:

Add near the top (after the `use` block):

```rust
use std::any::Any;
use std::sync::Arc;
```

Add before `SandboxedCommand`:

```rust
/// Opaque, backend-owned prepared execution plan riding on a
/// [`SandboxedCommand`] from `prepare()` to the runner (spec r3 D7).
///
/// In-process only — never serialized. Only the backend that minted it can
/// (and should) downcast it back. `Debug` prints a placeholder so command
/// logging cannot leak plan internals.
#[derive(Clone)]
pub struct BackendPlanHandle(Arc<dyn Any + Send + Sync>);

impl BackendPlanHandle {
    /// Wrap a backend plan value.
    #[must_use]
    pub fn new<T: Any + Send + Sync>(plan: T) -> Self {
        Self(Arc::new(plan))
    }

    /// Recover the concrete plan type. Returns `None` when the handle holds
    /// a different type (runners treat that as a malformed plan).
    #[must_use]
    pub fn downcast<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        self.0.clone().downcast::<T>().ok()
    }
}

impl std::fmt::Debug for BackendPlanHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BackendPlanHandle(..)")
    }
}
```

Change the `SandboxedCommand` struct and its impl:

```rust
#[derive(Debug, Clone)]
pub struct SandboxedCommand {
    inner: ProcessCommand,
    tag: SandboxedTag,
    plan: Option<BackendPlanHandle>,
}
```

In `impl SandboxedCommand`, set `plan: None` inside the existing `__new_sandboxed` constructor body (`Self { inner, tag, plan: None }`) and add:

```rust
    /// INTERNAL constructor for backends that carry a prepared plan to their
    /// runner (Android). Same visibility convention as [`__new_sandboxed`].
    ///
    /// [`__new_sandboxed`]: SandboxedCommand::__new_sandboxed
    #[must_use]
    pub fn __new_sandboxed_with_plan(
        inner: ProcessCommand,
        tag: SandboxedTag,
        plan: BackendPlanHandle,
    ) -> Self {
        Self {
            inner,
            tag,
            plan: Some(plan),
        }
    }

    /// The backend-owned prepared plan, when the minting backend attached one.
    #[must_use]
    pub fn backend_plan(&self) -> Option<&BackendPlanHandle> {
        self.plan.as_ref()
    }
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p traits && cargo check --workspace`
Expected: PASS / clean — `__new_sandboxed`'s signature did not change, so the posix/windows/hooks/tasks call sites are untouched.

- [ ] **Step 5: Commit**

```bash
git add traits/src/sandbox.rs
git commit -m "feat(traits): BackendPlanHandle plan carriage on SandboxedCommand (spec D7)"
```

---

### Task 4: Android plan types (`policy.rs` skeleton)

**Files:**
- Create: `platforms/android/src/policy.rs`
- Modify: `platforms/android/src/lib.rs` (module decl + re-exports)

- [ ] **Step 1: Declare the module** — in `platforms/android/src/lib.rs` after the `use` block add:

```rust
pub mod policy;

pub use policy::{
    AndroidSandboxPlan, ExecTarget, NetProfile, ProcessCleanup, Rlimit, RlimitResource, SeccompRef,
};
```

- [ ] **Step 2: Write the failing test** — create `platforms/android/src/policy.rs` with the test first:

```rust
//! `SandboxPolicy` → [`AndroidSandboxPlan`] mapping (spec r3 §Policy mapping).
//!
//! Pure functions, no I/O: validation/планning happens in `sandbox.rs`,
//! execution in `process.rs`. Fail-closed: every unenforceable request is
//! rejected with a named guarantee.

use std::path::PathBuf;

#[cfg(test)]
mod tests {
    use super::*;

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
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p platform-android plan_types_construct`
Expected: FAIL — types not defined

- [ ] **Step 4: Implement the types** — above the tests module in `policy.rs`:

```rust
/// What the runner will ultimately `execve` (spec r3 §AndroidSandboxPlan).
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

/// Network stance compiled into the jail (spec D10: Shell is DenyNet always;
/// only structured tools may build AllowNet plans).
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
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p platform-android plan_types_construct`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add platforms/android/src/policy.rs platforms/android/src/lib.rs
git commit -m "feat(platform-android): android sandbox plan types (spec r3)"
```

---

### Task 5: fail-closed policy mapping (`plan_from_policy`)

**Files:**
- Modify: `platforms/android/src/policy.rs`

- [ ] **Step 1: Write the failing tests** — append inside `mod tests`:

```rust
    use traits::{NetworkPolicy, ResourceLimits, SandboxError, SandboxPolicy};

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
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p platform-android policy::`
Expected: FAIL — `plan_from_policy` not found

- [ ] **Step 3: Implement** — add to `policy.rs` (above tests):

```rust
use traits::{NetworkPolicy, SandboxError, SandboxPolicy};

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
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p platform-android policy::`
Expected: PASS (6 tests)

- [ ] **Step 5: Commit**

```bash
git add platforms/android/src/policy.rs
git commit -m "feat(platform-android): fail-closed SandboxPolicy->AndroidSandboxPlan mapping"
```

---

### Task 6: environment scrub + allowlist rebuild

**Files:**
- Modify: `platforms/android/src/policy.rs` (env builder lives with the mapping)
- Depends on: Task 7's `AndroidShellConfig` — to keep tasks independently readable the builder takes plain paths, not the config struct.

- [ ] **Step 1: Failing tests** — append inside `mod tests`:

```rust
    use std::collections::HashMap;

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
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p platform-android env_is_scrubbed`
Expected: FAIL — `build_shell_env` not found

- [ ] **Step 3: Implement** — add to `policy.rs`:

```rust
use std::collections::HashMap;
use std::path::Path;

/// Build the child environment: the inherited environment is DISCARDED
/// entirely and rebuilt from the spec's allowlist table (spec r3 §Environment);
/// `caller_env` (the tool's explicit `cmd.env`) overlays last — explicit wins.
#[must_use]
pub fn build_shell_env(
    workspace_root: &Path,
    cache_dir: &Path,
    bundled_helper_dir: Option<&Path>,
    caller_env: &HashMap<String, String>,
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
        if let Some(slot) = env.iter_mut().find(|(name, _)| name == k) {
            slot.1 = v.clone();
        } else {
            env.push((k.clone(), v.clone()));
        }
    }
    env
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p platform-android policy::`
Expected: PASS (8 tests)

- [ ] **Step 5: Commit**

```bash
git add platforms/android/src/policy.rs
git commit -m "feat(platform-android): shell env scrub + allowlist rebuild"
```

---

### Task 7: `AndroidShellConfig` + platform inputs threading

**Files:**
- Create: `platforms/android/src/config.rs`
- Modify: `platforms/android/src/lib.rs` (module decl, `AndroidPlatformInputs.shell`, constructor)
- Modify: `apps/android-aar/src/lib.rs:110-119` and `apps/android-aar/src/lib.rs:1012-1023` (struct-literal fix-ups)

- [ ] **Step 1: Create `platforms/android/src/config.rs`:**

```rust
//! Host-supplied (Kotlin → Rust) shell configuration (spec r3 §Android inputs).

use std::path::PathBuf;

/// Android shell/sandbox inputs. Provided explicitly by the Kotlin bootstrap —
/// shell support is never enabled by target OS alone.
#[derive(Debug, Clone)]
pub struct AndroidShellConfig {
    /// `ApplicationInfo.nativeLibraryDir` — the only legal root for bundled
    /// helper executables (P4+).
    pub native_library_dir: PathBuf,
    /// The directory the shell treats as `$HOME` / the workspace. A product
    /// boundary, not an OS boundary (spec Threat model).
    pub shell_workspace_root: PathBuf,
    /// App cache dir → `$TMPDIR`.
    pub app_cache_root: PathBuf,
    /// Application package name.
    pub package_name: String,
    /// `PackageInfo.longVersionCode` — capability-cache key component.
    pub package_version_code: i64,
    /// Roots that must never contain an exec target (filesDir, cacheDir,
    /// codeCacheDir, noBackupFilesDir, extracted-assets roots).
    pub app_writable_roots: Vec<PathBuf>,
    /// Master enable flag (registration gate #3).
    pub enable_shell: bool,
    /// D11: host attests no plaintext secrets live under shell-readable
    /// app-private paths (Keystore migration done).
    pub secrets_in_keystore: bool,
    /// D11: explicit user acceptance of the data-exposure reality.
    pub shell_data_exposure_accepted: bool,
}

impl AndroidShellConfig {
    /// D11 secrets gate (registration gate #4): Keystore attestation OR
    /// explicit user acceptance.
    #[must_use]
    pub fn secrets_gate_satisfied(&self) -> bool {
        self.secrets_in_keystore || self.shell_data_exposure_accepted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(keystore: bool, accepted: bool) -> AndroidShellConfig {
        AndroidShellConfig {
            native_library_dir: PathBuf::from("/data/app/x/lib/arm64"),
            shell_workspace_root: PathBuf::from("/data/user/0/x/files/ws"),
            app_cache_root: PathBuf::from("/data/user/0/x/cache"),
            package_name: "com.example".into(),
            package_version_code: 42,
            app_writable_roots: vec![PathBuf::from("/data/user/0/x/files")],
            enable_shell: true,
            secrets_in_keystore: keystore,
            shell_data_exposure_accepted: accepted,
        }
    }

    #[test]
    fn secrets_gate_requires_keystore_or_acceptance() {
        assert!(!cfg(false, false).secrets_gate_satisfied());
        assert!(cfg(true, false).secrets_gate_satisfied());
        assert!(cfg(false, true).secrets_gate_satisfied());
    }
}
```

- [ ] **Step 2: Thread through `AndroidPlatformInputs`** — in `platforms/android/src/lib.rs`:

Add module decl + re-export next to the Task 4 ones:

```rust
pub mod config;

pub use config::AndroidShellConfig;
```

Add a field at the end of `AndroidPlatformInputs` (after `clipboard`):

```rust
    /// Android shell/sandbox configuration (spec r3). `None` keeps shell
    /// support fully absent (posix-minimal stubs stay wired).
    pub shell: Option<AndroidShellConfig>,
}
```

In `AndroidPlatform::new`, the constructor body does not use it yet (Task 11 wires it); consume it to keep clippy quiet:

```rust
        // Task 11 swaps the sandbox/process handles when `shell` is Some.
        let _ = &inputs.shell;
```

- [ ] **Step 3: Fix the two struct-literal call sites** — in `apps/android-aar/src/lib.rs`, both `AndroidPlatformInputs { ... }` literals (the `build_mobile_engine` one at ~line 110 and the `build_android_engine` one at ~line 1012) gain:

```rust
            shell: None,
```

- [ ] **Step 4: Verify**

Run: `cargo test -p platform-android config:: && cargo check --workspace`
Expected: PASS / clean

- [ ] **Step 5: Commit**

```bash
git add platforms/android/src/config.rs platforms/android/src/lib.rs apps/android-aar/src/lib.rs
git commit -m "feat(platform-android): AndroidShellConfig with D11 secrets gate"
```

---

### Task 8: capability cache + probe seam

**Files:**
- Create: `platforms/android/src/capabilities.rs`
- Modify: `platforms/android/src/lib.rs` (module decl + re-export)

- [ ] **Step 1: Declare module** — in `platforms/android/src/lib.rs`:

```rust
pub mod capabilities;

pub use capabilities::{AndroidSandboxCapabilities, CapabilityCache};
```

- [ ] **Step 2: Create `platforms/android/src/capabilities.rs`** (tests included — host behavior is fully specified, the on-device probe body lands in P0a Task 16 / P2):

```rust
//! One-shot capability probe + session cache (spec r3 §Capability probing).
//!
//! Eager (D8): `engine_mobile::build_mobile_engine` runs the probe via
//! `block_on` BEFORE the synchronous tool registry is assembled; everything
//! downstream (prepare(), registration gates) reads the cache only.

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
        self.0
            .get()
            .cloned()
            .unwrap_or_else(|| AndroidSandboxCapabilities::unavailable("capability probe has not run"))
    }
}

/// Run the capability probe.
///
/// Host (non-Android) builds: the sandbox is structurally absent — return
/// the conservative result so host tests exercise the gates.
/// Android builds: P0a Task 16 wires the minijail smoke; the remaining probe
/// items land with the P2 runner.
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
```

(`platform-android/Cargo.toml` needs `[dev-dependencies] tokio = { workspace = true, features = ["macros", "rt"] }` for the async test.)

- [ ] **Step 3: Verify**

Run: `cargo test -p platform-android capabilities::`
Expected: PASS (3 tests)

- [ ] **Step 4: Commit**

```bash
git add platforms/android/src/capabilities.rs platforms/android/src/lib.rs platforms/android/Cargo.toml
git commit -m "feat(platform-android): capability probe seam + conservative cache"
```

---

### Task 9: `AndroidMinijailSandbox` (prepare)

**Files:**
- Create: `platforms/android/src/sandbox.rs`
- Modify: `platforms/android/src/lib.rs` (module decl + re-export)

- [ ] **Step 1: Declare module** — in `platforms/android/src/lib.rs`:

```rust
pub mod sandbox;

pub use sandbox::AndroidMinijailSandbox;
```

- [ ] **Step 2: Create `platforms/android/src/sandbox.rs`** with tests first (then run, then implementation — shown together here; the executor writes tests, sees them fail, then adds the impl):

Tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::AndroidSandboxCapabilities;
    use crate::policy::NetProfile; // not in the impl's imports — tests need it explicitly
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

    fn sandbox_with(ws: &std::path::Path, caps: AndroidSandboxCapabilities) -> AndroidMinijailSandbox {
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
```

Implementation (above the tests):

```rust
//! [`traits::Sandbox`] impl for Android — validation + plan construction only;
//! never spawns (spec r3 §AndroidMinijailSandbox).

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use traits::{
    BackendPlanHandle, ProcessCommand, Sandbox, SandboxBackend, SandboxCapability, SandboxError,
    SandboxPolicy, SandboxedCommand, SandboxedTag,
};

use crate::capabilities::CapabilityCache;
use crate::config::AndroidShellConfig;
use crate::policy::{build_shell_env, plan_from_policy, AndroidSandboxPlan, ExecTarget};

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

        let cwd = self.resolve_cwd(cmd.cwd.as_ref())?;

        let env = build_shell_env(
            &cwd,
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
            cwd: Some(cwd),
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
```

(`platform-android/Cargo.toml` gains `async-trait = { workspace = true }` under `[dependencies]` and `tempfile = { workspace = true }` under `[dev-dependencies]`.)

- [ ] **Step 3: Run the failing tests, implement, re-run**

Run: `cargo test -p platform-android sandbox::`
Expected: 7 tests PASS

- [ ] **Step 4: Commit**

```bash
git add platforms/android/src/sandbox.rs platforms/android/src/lib.rs platforms/android/Cargo.toml
git commit -m "feat(platform-android): AndroidMinijailSandbox prepare with fail-closed plan attach"
```

---

### Task 10: runner security invariants (execution disabled)

**Files:**
- Create: `platforms/android/src/process.rs`
- Modify: `platforms/android/src/lib.rs` (module decl + re-export)

- [ ] **Step 1: Declare module** — in `platforms/android/src/lib.rs`:

```rust
pub mod process;

pub use process::AndroidMinijailProcessRunner;
```

- [ ] **Step 2: Create `platforms/android/src/process.rs`** (tests first, then impl):

Tests:

```rust
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
        let plan = crate::policy::AndroidSandboxPlan {
            target: crate::policy::ExecTarget::SystemShell,
            argv: vec!["sh".into()],
            env: vec![],
            network: crate::policy::NetProfile::DenyNet,
            rlimits: vec![],
            seccomp_policy: None,
            cleanup: crate::policy::ProcessCleanup::KillProcessGroup,
        };
        let sc = SandboxedCommand::__new_sandboxed_with_plan(
            cmd(),
            SandboxedTag::Wrapped {
                backend: SandboxBackend::AndroidMinijail,
            },
            BackendPlanHandle::new(plan),
        );
        let err = runner().run(&sc).await.unwrap_err();
        assert!(matches!(err, ProcessError::Unsupported));
        assert!(!runner().is_available());
    }
}
```

Implementation:

```rust
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

    /// The three security invariants (spec r3 §AndroidMinijailProcessRunner):
    /// reject bypass tags, foreign backends, and missing/foreign plans.
    fn admitted_plan(cmd: &SandboxedCommand) -> Result<Arc<AndroidSandboxPlan>, ProcessError> {
        match cmd.tag() {
            SandboxedTag::BypassAuditedWithReason { .. } => {
                return Err(ProcessError::MalformedSandboxPlan(
                    "bypass-audited commands cannot execute on Android (security invariant #1)"
                        .into(),
                ));
            }
            SandboxedTag::Wrapped { backend } if *backend != SandboxBackend::AndroidMinijail => {
                return Err(ProcessError::MalformedSandboxPlan(format!(
                    "foreign sandbox backend {backend:?} (security invariant #2)"
                )));
            }
            SandboxedTag::Wrapped { .. } => {}
        }
        let handle = cmd.backend_plan().ok_or_else(|| {
            ProcessError::MalformedSandboxPlan(
                "android plan missing from SandboxedCommand (security invariant #3)".into(),
            )
        })?;
        handle.downcast::<AndroidSandboxPlan>().ok_or_else(|| {
            ProcessError::MalformedSandboxPlan(
                "backend plan failed to downcast to AndroidSandboxPlan".into(),
            )
        })
    }
}

#[async_trait]
impl ProcessRunner for AndroidMinijailProcessRunner {
    async fn run(&self, cmd: &SandboxedCommand) -> Result<ProcessOutput, ProcessError> {
        let _plan = Self::admitted_plan(cmd)?;
        // P2 (gated on P0a): spawn_blocking → minijail_run_pid_pipes.
        Err(ProcessError::Unsupported)
    }

    async fn spawn_background(
        &self,
        _cmd: &SandboxedCommand,
    ) -> Result<ProcessHandle, ProcessError> {
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
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p platform-android process::`
Expected: 4 tests PASS

- [ ] **Step 4: Commit**

```bash
git add platforms/android/src/process.rs platforms/android/src/lib.rs
git commit -m "feat(platform-android): runner security invariants (execution disabled until P2)"
```

---

### Task 11: wire shell config into `AndroidPlatform::new`

**Files:**
- Modify: `platforms/android/src/lib.rs:71-94` (constructor)

- [ ] **Step 1: Failing test** — add at the bottom of `platforms/android/src/lib.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use traits::{
        CameraControl, CameraError, CapturePhotoOpts, CapturedImage, Platform, SandboxBackend,
        ShareError, SharePayload, ShareResult, SharingService, VoiceError, VoiceRecorder,
        VoiceRecording, VoiceRecordingOpts,
    };

    struct NoCam;
    #[async_trait]
    impl CameraControl for NoCam {
        async fn capture_photo(&self, _: CapturePhotoOpts) -> Result<CapturedImage, CameraError> {
            Err(CameraError::DeviceUnavailable)
        }
        async fn pick_from_library(&self) -> Result<CapturedImage, CameraError> {
            Err(CameraError::DeviceUnavailable)
        }
    }
    struct NoVoice;
    #[async_trait]
    impl VoiceRecorder for NoVoice {
        async fn start_recording(&self, _: VoiceRecordingOpts) -> Result<(), VoiceError> {
            Err(VoiceError::NotRecording)
        }
        async fn stop_recording(&self) -> Result<VoiceRecording, VoiceError> {
            Err(VoiceError::NotRecording)
        }
        async fn is_recording(&self) -> bool {
            false
        }
    }
    struct NoShare;
    #[async_trait]
    impl SharingService for NoShare {
        async fn share(&self, _: SharePayload) -> Result<ShareResult, ShareError> {
            Err(ShareError::Unsupported)
        }
    }

    fn inputs(shell: Option<AndroidShellConfig>) -> AndroidPlatformInputs {
        AndroidPlatformInputs {
            app_files_root: std::env::temp_dir(),
            camera: std::sync::Arc::new(NoCam),
            voice: std::sync::Arc::new(NoVoice),
            share: std::sync::Arc::new(NoShare),
            stt: None,
            tts: None,
            notifications: None,
            clipboard: None,
            shell,
        }
    }

    fn shell_cfg() -> AndroidShellConfig {
        AndroidShellConfig {
            native_library_dir: std::env::temp_dir(),
            shell_workspace_root: std::env::temp_dir(),
            app_cache_root: std::env::temp_dir(),
            package_name: "com.example".into(),
            package_version_code: 1,
            app_writable_roots: vec![],
            enable_shell: true,
            secrets_in_keystore: true,
            shell_data_exposure_accepted: false,
        }
    }

    #[test]
    fn shell_config_wires_android_sandbox_and_runner() {
        let p = AndroidPlatform::new(inputs(Some(shell_cfg())));
        assert_eq!(p.sandbox().backend(), SandboxBackend::AndroidMinijail);
        assert!(!p.process().is_available(), "execution disabled until P2");
    }

    #[test]
    fn no_shell_config_keeps_posix_minimal_stubs() {
        let p = AndroidPlatform::new(inputs(None));
        assert_eq!(p.sandbox().backend(), SandboxBackend::None);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p platform-android shell_config_wires`
Expected: FAIL — backend is `None` (stubs still wired)

- [ ] **Step 3: Implement** — in `AndroidPlatform::new`, replace the fixed `process`/`sandbox` lines and drop the Task 7 `let _ = &inputs.shell;`:

```rust
        let (process, sandbox): (Arc<dyn ProcessRunner>, Arc<dyn Sandbox>) = match inputs.shell {
            Some(shell_cfg) => {
                let caps = Arc::new(crate::capabilities::CapabilityCache::new());
                (
                    Arc::new(crate::process::AndroidMinijailProcessRunner::new()),
                    Arc::new(crate::sandbox::AndroidMinijailSandbox::new(shell_cfg, caps)),
                )
            }
            None => (
                Arc::new(PosixProcess::new()) as Arc<dyn ProcessRunner>,
                Arc::new(PosixSandbox::new()) as Arc<dyn Sandbox>,
            ),
        };
```

and use `process` / `sandbox` in the `Self { ... }` literal instead of the inline `Arc::new(...)` calls.

(The capability cache created here is the same instance `probe_android_capabilities` must fill in the engine-mobile eager-probe wiring — that wiring belongs to the P2/P3 plans; in P1 the cache reads conservative-unavailable, which is exactly the fail-closed default.)

- [ ] **Step 4: Run tests**

Run: `cargo test -p platform-android && cargo check --workspace`
Expected: PASS / clean

- [ ] **Step 5: Commit**

```bash
git add platforms/android/src/lib.rs
git commit -m "feat(platform-android): wire AndroidMinijail sandbox/runner behind shell config"
```

---

### Task 12: FFI plumbing (`AndroidShellConfigFfi`)

**Files:**
- Modify: `apps/android-aar/src/lib.rs` (new record + `build_android_engine` param at lines 980-994 + both branch bodies)

- [ ] **Step 1: Add the UniFFI record** — near `PlatformImpls` in `apps/android-aar/src/lib.rs`:

```rust
/// FFI carrier for the Android shell/sandbox configuration (spec r3 §Android
/// inputs). `None` anywhere upstream keeps shell support fully absent.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct AndroidShellConfigFfi {
    /// `ApplicationInfo.nativeLibraryDir`.
    pub native_library_dir: String,
    /// Directory the shell treats as `$HOME` / workspace.
    pub shell_workspace_root: String,
    /// App cache dir (`$TMPDIR`).
    pub app_cache_root: String,
    /// Application package name.
    pub package_name: String,
    /// `PackageInfo.longVersionCode`.
    pub package_version_code: i64,
    /// filesDir / cacheDir / codeCacheDir / noBackupFilesDir roots.
    pub app_writable_roots: Vec<String>,
    /// Master enable flag.
    pub enable_shell: bool,
    /// D11: host attests secrets are Keystore-backed.
    pub secrets_in_keystore: bool,
    /// D11: explicit user acceptance of data exposure.
    pub shell_data_exposure_accepted: bool,
}
```

- [ ] **Step 2: Thread the parameter** — `build_android_engine` gains a final parameter:

```rust
    shell: Option<AndroidShellConfigFfi>,
```

In the `#[cfg(target_os = "android")]` branch, before the `AndroidPlatformInputs` literal:

```rust
        let shell_cfg = shell.map(|s| platform_android::AndroidShellConfig {
            native_library_dir: std::path::PathBuf::from(s.native_library_dir),
            shell_workspace_root: std::path::PathBuf::from(s.shell_workspace_root),
            app_cache_root: std::path::PathBuf::from(s.app_cache_root),
            package_name: s.package_name,
            package_version_code: s.package_version_code,
            app_writable_roots: s.app_writable_roots.into_iter().map(Into::into).collect(),
            enable_shell: s.enable_shell,
            secrets_in_keystore: s.secrets_in_keystore,
            shell_data_exposure_accepted: s.shell_data_exposure_accepted,
        });
```

and in the inputs literal replace `shell: None,` with `shell: shell_cfg,`. In the non-android branch, add `shell` to the existing `let _ = (...)` tuple. The older non-exported `build_mobile_engine` wrapper keeps `shell: None`.

- [ ] **Step 3: Check for Kotlin call sites**

Run: `grep -rn "buildAndroidEngine\|build_android_engine" apps/android-aar/kotlin clients/android --include="*.kt" | head`
Expected: list of call sites (possibly empty). For each, append `null` as the new `shell` argument.

- [ ] **Step 4: Verify**

Run: `cargo test -p android-aar && cargo check --workspace`
Expected: PASS / clean (host path still returns `PlatformUnavailable`)

- [ ] **Step 5: Commit**

```bash
git add apps/android-aar/src/lib.rs apps/android-aar/kotlin
git commit -m "feat(android-aar): AndroidShellConfigFfi plumbing through build_android_engine"
```

---

### Task 13: P1 gate — full verification

- [ ] **Step 1: Format + lint + test**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
Expected: all green. Fix anything that surfaces (e.g. missing docs on new public items — every new `pub` item above already carries a doc comment).

- [ ] **Step 2: Commit any fixups**

```bash
git add -A lingxi-code && git commit -m "chore(android-sandbox): P1 gate fixups" || echo "nothing to fix"
```

---

# Part 2 — P0a: libminijail NDK build + on-device smoke (global gate for P2+)

> P0a failure = STOP. Do not start the P2 plan until Task 17's instrumentation smoke passes on a device/emulator. None of this part installs an executable — libminijail enters as a library (W^X-exempt).

### Task 14: NDK preflight + vendored libcap

**Files:**
- Create: `third_party/libcap/` (vendored source)
- Create: `lingxi-code/platforms/android-libcap/` (`Cargo.toml`, `build.rs`, `src/lib.rs`, `gen/cap_names.h`)
- Modify: `lingxi-code/Cargo.toml` (workspace members += `platforms/android-libcap`)

- [ ] **Step 1: Preflight the toolchain**

Run: `ls "$ANDROID_NDK_HOME"/toolchains/llvm/prebuilt/*/bin/aarch64-linux-android29-clang 2>/dev/null || ls ~/Library/Android/sdk/ndk/*/toolchains/llvm/prebuilt/*/bin/aarch64-linux-android29-clang | tail -1`
Expected: one clang path prints. Export it for later steps: `export NDK_CLANG=<that path>`. Also: `cargo ndk --version` (install with `cargo install cargo-ndk` if missing).

- [ ] **Step 2: Vendor libcap** (minijail links `-lcap`; the NDK ships none)

```bash
git clone --depth 1 --branch libcap-2.69 \
  https://git.kernel.org/pub/scm/libs/libcap/libcap.git third_party/libcap
```

Expected: `third_party/libcap/libcap/cap_proc.c` exists.

- [ ] **Step 3: Pre-generate `cap_names.h` on the host** (libcap generates it with a host tool; we commit the output so the Android build never runs host programs)

```bash
make -C third_party/libcap/libcap cap_names.h GOLANG=no
mkdir -p lingxi-code/platforms/android-libcap/gen
cp third_party/libcap/libcap/cap_names.h lingxi-code/platforms/android-libcap/gen/
```

Expected: `gen/cap_names.h` starts with `{"chown",0},`-style entries.

- [ ] **Step 4: Create the crate** — `lingxi-code/platforms/android-libcap/Cargo.toml`:

```toml
[package]
name = "platform-android-libcap"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true
build = "build.rs"
links = "cap"

# Static libcap for aarch64-linux-android — a build-only crate: it compiles
# vendored libcap sources and emits link flags; the lib.rs is empty. libcap is
# BSD-3-Clause OR GPL-2.0 (we use the BSD option; see OSS notices).

[build-dependencies]
cc = "1"

[lints]
workspace = true
```

`src/lib.rs`:

```rust
//! Build-only crate: compiles vendored libcap (`third_party/libcap`) to a
//! static archive for Android targets and emits `cargo:rustc-link-lib=static=cap`.
//! No Rust API — minijail's C code is the consumer.
#![forbid(unsafe_code)]
```

`build.rs`:

```rust
use std::env;
use std::path::PathBuf;

fn main() {
    // Only Android targets get a real build; host builds emit nothing so the
    // workspace stays checkable on macOS/Linux.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("android") {
        return;
    }
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let libcap_src = manifest
        .ancestors()
        .nth(3) // platforms/android-libcap -> platforms -> lingxi-code -> repo root
        .unwrap()
        .join("third_party/libcap/libcap");

    cc::Build::new()
        .files(
            ["cap_alloc.c", "cap_proc.c", "cap_extint.c", "cap_flag.c", "cap_text.c", "cap_file.c"]
                .iter()
                .map(|f| libcap_src.join(f)),
        )
        .include(libcap_src.join("include"))
        .include(libcap_src.join("include/uapi"))
        .include(manifest.join("gen")) // pre-generated cap_names.h
        .define("_GNU_SOURCE", None)
        .warnings(false)
        .compile("cap");
    println!("cargo:rerun-if-changed={}", libcap_src.display());
}
```

Workspace `lingxi-code/Cargo.toml`: add `"platforms/android-libcap"` to `[workspace] members`.

- [ ] **Step 5: Verify host no-op + Android compile**

Run: `cargo check -p platform-android-libcap && cargo ndk -t arm64-v8a check -p platform-android-libcap`
Expected: both clean (the second compiles libcap with NDK clang; if a libcap source file fails under NDK headers, drop `cap_file.c` from the list — it is only needed for file-cap APIs minijail does not call — and note it in the commit message).

- [ ] **Step 6: Commit**

```bash
git add third_party/libcap lingxi-code/platforms/android-libcap lingxi-code/Cargo.toml
git commit -m "feat(android): vendored static libcap for the NDK minijail build (P0a)"
```

---

### Task 15: minijail-sys under the NDK

**Files:**
- Modify: `lingxi-code/platforms/android/Cargo.toml` — no; minijail stays OUT of `platform-android` (it is `forbid(unsafe_code)`). Instead:
- Create: `lingxi-code/platforms/android-minijail/` (`Cargo.toml`, `src/lib.rs`)
- Modify: `lingxi-code/Cargo.toml` (workspace member)

- [ ] **Step 1: Read the rest of the upstream build script** (its first 60 lines are pkg-config probe + cross-prefix helper):

Run: `sed -n '60,160p' ../third_party/minijail/rust/minijail-sys/build.rs`
Expected output to look for: the static-build fallback — which `make` invocation, which env it honors (`CROSS_COMPILE`, `CC`, `OUT_DIR`), and the bindgen call honoring `MINIJAIL_BINDGEN_TARGET`. Note the answers in the commit message.

- [ ] **Step 2: Create the wrapper crate** — `lingxi-code/platforms/android-minijail/Cargo.toml`:

```toml
[package]
name = "platform-android-minijail"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

# The ONLY crate that touches minijail FFI (platform-android is
# forbid(unsafe_code)). Wraps the upstream safe `minijail` crate and exposes
# the P0a smoke + (P2) the jailed-spawn entry point.

[target.'cfg(target_os = "android")'.dependencies]
minijail = { path = "../../../third_party/minijail/rust/minijail" }
libc = { workspace = true }
platform-android-libcap = { path = "../android-libcap" }

[dependencies]
serde = { workspace = true, features = ["derive"] }

[lints]
workspace = true
```

`src/lib.rs`:

```rust
//! Minijail FFI wrapper (P0a): the smoke probe proving libminijail links and
//! a jailed `sh -c true` survives on-device. The P2 plan adds the full
//! `minijail_run_pid_pipes` spawn path here.

use serde::Serialize;

/// Result of the on-device minijail smoke (serialized to the instrumentation
/// test through the `android_sandbox_smoke()` UniFFI export).
#[derive(Debug, Clone, Serialize)]
pub struct SmokeResult {
    /// Overall pass.
    pub ok: bool,
    /// `no_new_privs` was applied.
    pub no_new_privs: bool,
    /// The jailed child ran and exited 0.
    pub child_exit_zero: bool,
    /// Failure detail when `ok == false`.
    pub reason: Option<String>,
}

/// Run the minijail smoke. Host builds report a structural "not android".
#[must_use]
pub fn minijail_smoke() -> SmokeResult {
    #[cfg(not(target_os = "android"))]
    {
        SmokeResult {
            ok: false,
            no_new_privs: false,
            child_exit_zero: false,
            reason: Some("minijail smoke requires an Android device".into()),
        }
    }
    #[cfg(target_os = "android")]
    {
        android_impl::smoke()
    }
}

#[cfg(target_os = "android")]
mod android_impl {
    use super::SmokeResult;
    use std::path::Path;

    pub(super) fn smoke() -> SmokeResult {
        // Safe-wrapper API (third_party/minijail/rust/minijail): construct a
        // jail, set no_new_privs, fork+exec `/system/bin/sh -c true` keeping
        // stdio fds, and wait for exit 0.
        let mut jail = match minijail::Minijail::new() {
            Ok(j) => j,
            Err(e) => {
                return SmokeResult {
                    ok: false,
                    no_new_privs: false,
                    child_exit_zero: false,
                    reason: Some(format!("Minijail::new failed: {e}")),
                }
            }
        };
        jail.no_new_privs();
        let pid = match jail.run(
            Path::new("/system/bin/sh"),
            &[0, 1, 2],
            &["sh", "-c", "true"],
        ) {
            Ok(pid) => pid,
            Err(e) => {
                return SmokeResult {
                    ok: false,
                    no_new_privs: true,
                    child_exit_zero: false,
                    reason: Some(format!("jail.run failed: {e}")),
                }
            }
        };
        let mut status: libc::c_int = 0;
        // SAFETY: plain waitpid on the pid minijail returned.
        let rc = unsafe { libc::waitpid(pid, &mut status, 0) };
        let exited_zero =
            rc == pid && libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0;
        SmokeResult {
            ok: exited_zero,
            no_new_privs: true,
            child_exit_zero: exited_zero,
            reason: if exited_zero {
                None
            } else {
                Some(format!("waitpid rc={rc} status={status}"))
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_smoke_reports_structurally_unavailable() {
        let r = minijail_smoke();
        assert!(!r.ok);
        assert!(r.reason.unwrap().contains("Android"));
    }
}
```

> If Step 1 revealed the safe crate's `run` signature differs (e.g. it is `run(&self, ...)` or named `run_remap`), adjust the call accordingly — `grep -n "pub fn" ../third_party/minijail/rust/minijail/src/lib.rs | head -30` lists the real API; the smoke must (a) set `no_new_privs`, (b) inherit stdio fds 0/1/2, (c) return the child pid.

Workspace `lingxi-code/Cargo.toml`: add `"platforms/android-minijail"` to members.

- [ ] **Step 3: Host check + cross-compile gate (THE P0a build proof)**

Run:
```bash
cargo test -p platform-android-minijail
export MINIJAIL_BINDGEN_TARGET=aarch64-linux-android29
cargo ndk -t arm64-v8a build -p platform-android-minijail 2>&1 | tail -20
```
Expected: host test PASS; the ndk build compiles libminijail (the upstream `minijail-sys` static fallback builds `libminijail.a` — CORE objects: `libminijail.o syscall_filter.o signal_handler.o bpf.o landlock_util.o util.o system.o syscall_wrapper.o config_parser.o libconstants.gen.o libsyscalls.gen.o`) and links against our static `cap`.

**If the upstream build.rs/make path fails under the NDK** (gcc-isms, host include leakage): replace the dependency with a self-built static lib — add to `platform-android-minijail` a `build.rs` using `cc` over the eleven CORE source files with the two generated tables produced by running `../third_party/minijail/gen_constants.sh` / `gen_syscalls.sh` with `CC="$NDK_CLANG"`, then keep the safe `minijail` crate but point its `minijail-sys` at the prebuilt archive via `MINIJAIL_DO_NOT_BUILD=1` + `cargo:rustc-link-search`. Record whichever path worked in the commit message.

- [ ] **Step 4: x86_64 (emulator ABI)**

Run: `cargo ndk -t x86_64 build -p platform-android-minijail 2>&1 | tail -5`
Expected: clean — the gen tables are per-arch; a failure here means the table generation hardcoded arm64 (fix: regenerate per `CARGO_CFG_TARGET_ARCH`).

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/platforms/android-minijail lingxi-code/Cargo.toml
git commit -m "feat(android): minijail NDK build proof — platform-android-minijail smoke crate (P0a)"
```

---

### Task 16: wire the smoke into the capability probe

**Files:**
- Modify: `lingxi-code/platforms/android/Cargo.toml` (android-target dep)
- Modify: `lingxi-code/platforms/android/src/capabilities.rs` (android probe body)

- [ ] **Step 1: Add the target-gated dependency** — `platforms/android/Cargo.toml`:

```toml
[target.'cfg(target_os = "android")'.dependencies]
platform-android-minijail = { path = "../android-minijail" }
```

(`platform-android` keeps `#![forbid(unsafe_code)]` — the unsafe lives in the dep.)

- [ ] **Step 2: Replace the android probe body** in `capabilities.rs`:

```rust
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
```

- [ ] **Step 3: Verify host + cross**

Run: `cargo test -p platform-android && cargo ndk -t arm64-v8a check -p platform-android`
Expected: PASS / clean

- [ ] **Step 4: Commit**

```bash
git add lingxi-code/platforms/android
git commit -m "feat(platform-android): minijail smoke feeds the capability probe (P0a)"
```

---

### Task 17: on-device instrumentation smoke (the P0a gate itself)

**Files:**
- Modify: `lingxi-code/apps/android-aar/src/lib.rs` (smoke export)
- Create: `lingxi-code/apps/android-aar/kotlin/src/androidTest/kotlin/com/lingxi/aar/SandboxSmokeTest.kt` (adjust the package path to the existing one under `kotlin/src/main/kotlin/com/…`)

- [ ] **Step 1: Export the smoke over UniFFI** — in `apps/android-aar/src/lib.rs`:

```rust
/// P0a gate probe: run the on-device minijail smoke and return it as JSON
/// (`{"ok":bool,"no_new_privs":bool,"child_exit_zero":bool,"reason":...}`).
/// Host builds report the structural reason.
#[cfg(feature = "uniffi")]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn android_sandbox_smoke() -> String {
    #[cfg(target_os = "android")]
    {
        serde_json::to_string(&platform_android_minijail::minijail_smoke())
            .unwrap_or_else(|e| format!("{{\"ok\":false,\"reason\":\"serialize: {e}\"}}"))
    }
    #[cfg(not(target_os = "android"))]
    {
        "{\"ok\":false,\"reason\":\"host build\"}".to_string()
    }
}
```

(`android-aar/Cargo.toml`: add `serde_json = { workspace = true }` and the android-target dep `platform-android-minijail = { path = "../../platforms/android-minijail" }`.)

- [ ] **Step 2: Kotlin instrumentation test** — create `SandboxSmokeTest.kt` next to the existing instrumentation sources (match the package of `kotlin/src/main/kotlin/com/...`):

```kotlin
package com.lingxi.aar // ← match the existing package

import androidx.test.ext.junit.runners.AndroidJUnit4
import org.json.JSONObject
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

/** P0a global gate: libminijail links, no_new_privs applies, and a jailed
 *  `sh -c true` exits 0 on this device. */
@RunWith(AndroidJUnit4::class)
class SandboxSmokeTest {
    @Test
    fun minijailSmokePasses() {
        val json = JSONObject(uniffi.android_aar.androidSandboxSmoke())
        // Keys are serde's default snake_case (SmokeResult has no rename_all).
        assertTrue("smoke failed: $json", json.getBoolean("ok"))
        assertTrue(json.getBoolean("no_new_privs"))
        assertTrue(json.getBoolean("child_exit_zero"))
    }
}
```

(If `kotlin/build.gradle.kts` lacks instrumentation deps, add `androidTestImplementation("androidx.test.ext:junit:1.2.1")` and `androidTestImplementation("androidx.test:runner:1.6.2")` plus `testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"`.)

- [ ] **Step 3: Build + run on a device/emulator** (requires hardware — record the result; this is the gate)

```bash
cd apps/android-aar/kotlin && ./gradlew connectedAndroidTest
```
Expected: `SandboxSmokeTest > minijailSmokePasses PASSED` on API 29+ emulator AND once on a physical device. **FAIL ⇒ STOP: P2+ plans are blocked until this passes; debug via `adb logcat` + the JSON `reason`.**

- [ ] **Step 4: Commit**

```bash
git add lingxi-code/apps/android-aar
git commit -m "feat(android-aar): on-device minijail smoke export + instrumentation gate (P0a)"
```

---

## Plan completion checklist (spec coverage)

- Spec P1 row: trait extensions (T1-T3) ✓, prepare + policy mapping (T4-T6, T9) ✓, eager-probe seam (T8; engine-mobile `block_on` wiring lands with P2/P3 plans where the cache instance crosses into `build_mobile_engine`) ✓, AndroidShellConfig + UniFFI migration (T7, T12) ✓, host tests (every task) ✓, execution disabled (T10-T11) ✓, missing-plan rejection tests (T10) ✓.
- Spec P0a row: NDK static libminijail+libcap into the engine `.so` (T14-T15) ✓, on-device smoke: fork child + `no_new_privs` (T16-T17) ✓; harmless-seccomp + net-deny probe items are explicitly deferred to the P2 plan alongside the filter itself (the spec lists them under the probe; the P0a gate per spec is build/link/smoke).
- Out of scope here (follow-on plans): P2 runner execution + remaining probes, P3 Shell tool + registration gates + engine-mobile eager probe call, P4 Git tool + P0b packaging, P5 bundled interpreter.
