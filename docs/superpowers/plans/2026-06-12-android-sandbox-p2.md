# Android Sandbox P2 Implementation Plan — known-helper foreground execution

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the P1 fail-closed skeleton into a working in-engine Minijail runner: `AndroidMinijailProcessRunner::run()` actually forks a jailed `/system/bin/sh -c <command>` with `no_new_privs` + rlimits + net-deny seccomp + a child-side `chdir`, captures stdout/stderr/exit, enforces a wall-clock timeout by killing the process group, and emits an honest `AndroidSandboxReceipt`. Plus: complete the capability probe (seccomp install, TSYNC, net-deny `socket()`==EPERM, pgid-kill, landlock ABI, sh version, toybox inventory) and wire the eager probe so the cache is populated before tool registration.

**Architecture:** Spec r3 `docs/superpowers/specs/2026-06-12-android-sandbox-shell-design.md` §AndroidMinijailProcessRunner + §Capability probing. All unsafe minijail FFI lives in `platform-android-minijail` (extends the P0a crate); `platform-android` stays `#![forbid(unsafe_code)]` and calls into it through the existing target-gated dep. The runner wraps the blocking jailed spawn in `spawn_blocking`; the blocking fn self-enforces the timeout via a watchdog that `kill(-pgid)`s past the deadline, so all libc/FFI stays in one crate and the `platform-android` side is a safe translation of `AndroidSandboxPlan` → a plain spec struct.

**Predecessor:** P0a+P1 merged to main at `e2f51067`. This branch (`android-sandbox-p2`) is cut from main. Execution was disabled in P1 (`run()` → `Unsupported` after invariants); P2 flips that on.

**Verification reality:** the runner body is `#[cfg(target_os = "android")]` and cannot execute on the macOS host. Host tests cover the safe translation layer (plan → spec, receipt construction, seccomp policy text generation + hashing, probe-result → capabilities mapping). The jailed-execution behavior is proven by **instrumentation tests on the API-34 arm64 emulator** (the P0a gate harness in `clients/android`), extended here. Every task states which gate applies.

**Spec invariants P2 must not break:**
- The P1 runner security invariants (`admitted_plan`: reject bypass tag / foreign backend / missing-or-wrong plan) stay first and unchanged — execution only happens after they pass.
- Fail closed with named guarantees; no silent downgrade. A DenyNet plan whose seccomp filter cannot be installed must FAIL, not run unconfined.
- `plan.argv`/`plan.env` are authoritative; `cmd.inner()` mirrors them for audit only.
- `platform-android` keeps `#![forbid(unsafe_code)]`.

---

## File structure

```text
lingxi-code/
├── platforms/android-minijail/src/
│   ├── lib.rs               MODIFY: re-export the new run module + spec/output types
│   ├── run.rs               CREATE (android-cfg): JailSpec/JailedOutput + run_jailed() (all FFI)
│   ├── seccomp.rs           CREATE: net-deny policy TEXT generation (host-testable, no FFI) + per-arch syscall lists
│   └── probe.rs             CREATE (android-cfg): the real on-device probe extras (FFI)
├── platforms/android/src/
│   ├── process.rs           MODIFY: run() builds JailSpec from plan, spawn_blocking → run_jailed, builds receipt
│   ├── receipt.rs           CREATE: AndroidSandboxReceipt + from(plan, output) (safe, host-testable)
│   ├── policy.rs            MODIFY: populate SeccompRef{name,hash} for DenyNet plans (hash the policy text)
│   ├── capabilities.rs      MODIFY: probe_android_capabilities() android body calls probe.rs; fields filled
│   └── lib.rs               MODIFY: re-export receipt; pass cwd into JailSpec
├── apps/engine-mobile/src/host.rs   MODIFY: eager block_on(probe) → set the platform's capability cache
└── clients/android/app/src/androidTest/java/com/lingxi/code/
    └── SandboxRunTest.kt    CREATE: on-device run/timeout/net-deny/exit-code instrumentation
```

Note: P1 left the capability cache unreachable from outside `AndroidMinijailSandbox`. **Task 1 closes that seam first** (it blocks the eager-probe wiring and the runner's own filter-gating).

---

### Task 1: expose the capability cache + share one instance across sandbox and runner

**Problem (carried from P1 review):** `AndroidPlatform::new` creates a fresh `CapabilityCache` inside the `Some(shell)` arm and moves it into `AndroidMinijailSandbox`; nothing else can reach it, so (a) the eager probe can't populate it and (b) the runner can't read `seccomp_filter`/`net_deny_verified` to gate DenyNet admission.

**Files:**
- Modify: `platforms/android/src/lib.rs`
- Modify: `platforms/android/src/process.rs`

- [ ] **Step 1: Write the failing test** — in `platforms/android/src/lib.rs` tests module add:

```rust
    #[test]
    fn shell_platform_exposes_shared_capability_cache() {
        let p = AndroidPlatform::new(inputs(Some(shell_cfg())));
        let cache = p
            .shell_capability_cache()
            .expect("shell platform exposes its cache");
        // Same instance the sandbox reads: setting it flips is_available().
        cache.set(crate::capabilities::AndroidSandboxCapabilities {
            probed: true,
            minijail_smoke: true,
            no_new_privs: true,
            ..crate::capabilities::AndroidSandboxCapabilities::default()
        });
        assert!(p.sandbox().is_available(), "sandbox reads the shared cache");
    }

    #[test]
    fn no_shell_platform_has_no_cache() {
        let p = AndroidPlatform::new(inputs(None));
        assert!(p.shell_capability_cache().is_none());
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p platform-android shell_platform_exposes`
Expected: FAIL — no method `shell_capability_cache`.

- [ ] **Step 3: Implement** — in `platforms/android/src/lib.rs`:

Add a field to `AndroidPlatform`:

```rust
    /// The shared capability cache when shell support is wired (`None` for the
    /// posix-minimal-stub configuration). Held so the eager probe (engine-mobile)
    /// and the runner can read/populate the SAME instance the sandbox reads.
    shell_caps: Option<std::sync::Arc<crate::capabilities::CapabilityCache>>,
```

In `new`, the `Some(shell_cfg)` arm builds ONE `Arc<CapabilityCache>` and shares it: pass a clone into both `AndroidMinijailSandbox::new(shell_cfg, caps.clone())` and `AndroidMinijailProcessRunner::new(caps.clone())` (Task 2 gives the runner that constructor — for THIS task the runner still uses `::new()`; thread the cache into the sandbox + store it on the struct, and leave a `// Task 2: runner gets caps.clone()` marker). Store `shell_caps: Some(caps)` on `Self`; the `None` arm stores `shell_caps: None`. Add:

```rust
    /// The shared shell capability cache, when shell support is wired.
    #[must_use]
    pub fn shell_capability_cache(
        &self,
    ) -> Option<std::sync::Arc<crate::capabilities::CapabilityCache>> {
        self.shell_caps.clone()
    }
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p platform-android && cargo check --workspace`
Expected: PASS / clean.

- [ ] **Step 5: Commit**

```bash
git add platforms/android/src/lib.rs
git commit -m "feat(platform-android): share + expose the shell capability cache (P2 seam)"
```

---

### Task 2: net-deny seccomp BPF builder + hash (host-testable, no FFI)

> **CORRECTION (verified against vendored source).** Minijail's seccomp *policy files* are an **allowlist with a default KILL/TRAP fall-through** (`third_party/minijail/syscall_filter.c:817-846`) — there is **no `@default ALLOW`** (the only `@` directives are `@include`/`@frequency`). A policy listing only socket syscalls would therefore KILL the shell on its first `read()`. The spec wants the opposite: allow-by-default, deny only socket-family with a clean EPERM. The correct path is a **hand-built raw BPF program** injected via `minijail_set_seccomp_filters(j, &fprog)` (libminijail.c:1597 — installs the caller's filter verbatim, does not own/modify it), paired with `minijail_use_seccomp_filter(j)`. Constraint: `set_seccomp_filters` **dies if `minijail_log_seccomp_filter_failures()` was called** — Task 4 must NOT log-and-set together.

The BPF program is pure data: validate arch, load the syscall nr, jump-equal each socket-family nr → `RET ERRNO(EPERM)`, default → `RET ALLOW`. Build + hash it on the host; convert to `sock_filter`/`sock_fprog` and inject on-device (Task 4). The per-arch socket syscall numbers and `AUDIT_ARCH` are passed IN (so the builder stays host-testable; on-device the caller supplies `libc::SYS_socket as u32`, … and the right `AUDIT_ARCH_*`).

**Files:**
- Create: `platforms/android-minijail/src/seccomp.rs`
- Modify: `platforms/android-minijail/src/lib.rs` (`pub mod seccomp;` + re-export)
- Modify: `platforms/android-minijail/Cargo.toml` (add `sha2` + `hex`; verify keys vs anthropic-oauth/api-client and mirror — `sha2 = "0.10"`, `hex = "0.4"` if not workspace deps)

- [ ] **Step 1: Write the failing tests** — create `seccomp.rs` with tests first:

```rust
//! Net-deny seccomp BPF (spec r3 §Policy mapping: `network = Disabled` ⇒ seccomp
//! deny of socket-family syscalls). A hand-built classic-BPF program: allow by
//! default, return EPERM for socket-family syscalls. Pure data here; converted
//! to `sock_fprog` and injected via `minijail_set_seccomp_filters` on-device
//! (`run.rs`). Minijail policy *files* are allowlist/default-KILL and cannot
//! express allow-by-default — hence the raw filter (see plan Task 2 CORRECTION).

#[cfg(test)]
mod tests {
    use super::*;

    // arm64 socket-family nrs (sample for the host test; on-device the caller
    // passes libc::SYS_* — see `socket_syscall_nrs()` in run.rs).
    const ARM64_SOCKET_NRS: &[u32] = &[198, 199, 200, 201, 202, 203, 206, 207, 212];
    const AUDIT_ARCH_AARCH64: u32 = 0xC000_00B7;

    #[test]
    fn bpf_validates_arch_loads_nr_and_denies_then_allows() {
        let prog = build_net_deny_bpf(ARM64_SOCKET_NRS, AUDIT_ARCH_AARCH64);
        // First insns load+check arch (offset 4 in seccomp_data), then load nr
        // (offset 0). Last insn is the default RET ALLOW.
        assert!(prog.len() >= ARM64_SOCKET_NRS.len() + 4, "arch+nr+jumps+rets");
        let last = prog.last().copied().unwrap();
        assert_eq!(last.code, BPF_RET_K, "default action is the final insn");
        assert_eq!(last.k, SECCOMP_RET_ALLOW, "default = ALLOW");
        // Exactly one ERRNO(EPERM) return present.
        assert!(
            prog.iter().any(|i| i.code == BPF_RET_K && i.k == seccomp_ret_errno(1)),
            "denied syscalls return EPERM(1)"
        );
        // A RET KILL must NOT appear — net denial is graceful, not fatal.
        assert!(!prog.iter().any(|i| i.code == BPF_RET_K && i.k == SECCOMP_RET_KILL));
    }

    #[test]
    fn bpf_hash_is_stable_hex_and_arch_sensitive() {
        let a = net_deny_bpf_hash(ARM64_SOCKET_NRS, AUDIT_ARCH_AARCH64);
        let b = net_deny_bpf_hash(ARM64_SOCKET_NRS, AUDIT_ARCH_AARCH64);
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        // Different arch ⇒ different program ⇒ different hash.
        let x86 = net_deny_bpf_hash(&[41, 42, 43, 49, 50], 0xC000_003E);
        assert_ne!(a, x86);
    }

    #[test]
    fn policy_name_is_versioned() {
        assert_eq!(net_deny_policy_name(), "net-deny-v1");
    }
}
```

- [ ] **Step 2: Run → FAIL** (`cargo test -p platform-android-minijail seccomp`).

- [ ] **Step 3: Implement** — above the tests. A `BpfInsn` mirrors `struct sock_filter` (host-portable plain data; `run.rs` transmutes/maps it to `libc::sock_filter`):

```rust
use sha2::{Digest, Sha256};

/// One classic-BPF instruction — the four fields of `struct sock_filter`.
/// Plain data so the program is built+hashed on the host; `run.rs` maps it to
/// `libc::sock_filter` on-device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BpfInsn {
    /// Opcode.
    pub code: u16,
    /// Jump-true offset.
    pub jt: u8,
    /// Jump-false offset.
    pub jf: u8,
    /// Generic field (immediate / return value).
    pub k: u32,
}

// Classic-BPF / seccomp constants (linux/bpf_common.h, linux/seccomp.h).
/// `BPF_LD | BPF_W | BPF_ABS` — load a 32-bit word from seccomp_data at `k`.
pub const BPF_LD_W_ABS: u16 = 0x20;
/// `BPF_JMP | BPF_JEQ | BPF_K` — jump if A == k.
pub const BPF_JEQ_K: u16 = 0x15;
/// `BPF_RET | BPF_K` — return constant k.
pub const BPF_RET_K: u16 = 0x06;
/// seccomp_data offsets: nr at 0, arch at 4.
const SECCOMP_DATA_NR_OFF: u32 = 0;
const SECCOMP_DATA_ARCH_OFF: u32 = 4;
/// `SECCOMP_RET_ALLOW`.
pub const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;
/// `SECCOMP_RET_KILL` (== SECCOMP_RET_KILL_THREAD 0x0000_0000).
pub const SECCOMP_RET_KILL: u32 = 0x0000_0000;
const SECCOMP_RET_ERRNO_BASE: u32 = 0x0005_0000;
const SECCOMP_RET_DATA: u32 = 0x0000_ffff;

/// `SECCOMP_RET_ERRNO | (errno & DATA)`.
#[must_use]
pub fn seccomp_ret_errno(errno: u32) -> u32 {
    SECCOMP_RET_ERRNO_BASE | (errno & SECCOMP_RET_DATA)
}

/// The policy name recorded in the receipt's `SeccompRef`.
#[must_use]
pub fn net_deny_policy_name() -> &'static str {
    "net-deny-v1"
}

/// Build the net-deny classic-BPF program for one architecture.
///
/// Shape: (1) load `arch`, kill if it isn't `audit_arch` (defeats the x86_64
/// vs x32 nr-aliasing trick); (2) load `nr`; (3) for each socket-family nr, if
/// equal return EPERM; (4) default return ALLOW. `socket_nrs` and `audit_arch`
/// are arch-specific and passed in by the on-device caller (`libc::SYS_*`,
/// `AUDIT_ARCH_*`) so this stays host-testable.
#[must_use]
pub fn build_net_deny_bpf(socket_nrs: &[u32], audit_arch: u32) -> Vec<BpfInsn> {
    let mut prog = Vec::new();
    // Arch guard: load arch; if != audit_arch jump to the KILL at the very end.
    // We compute the KILL offset after building the body, so emit a placeholder
    // jump and patch — simplest is: arch-mismatch → next insn is RET KILL.
    prog.push(BpfInsn { code: BPF_LD_W_ABS, jt: 0, jf: 0, k: SECCOMP_DATA_ARCH_OFF });
    // if arch == audit_arch, skip the next (KILL) insn.
    prog.push(BpfInsn { code: BPF_JEQ_K, jt: 1, jf: 0, k: audit_arch });
    prog.push(BpfInsn { code: BPF_RET_K, jt: 0, jf: 0, k: SECCOMP_RET_KILL });
    // Load the syscall nr.
    prog.push(BpfInsn { code: BPF_LD_W_ABS, jt: 0, jf: 0, k: SECCOMP_DATA_NR_OFF });
    // For each socket nr: if nr == sc, jump to the ERRNO ret (placed right after
    // the comparison chain). Compute the chain so each match jumps to the ERRNO
    // insn which sits immediately before the final ALLOW.
    let n = socket_nrs.len() as u8;
    for (i, nr) in socket_nrs.iter().enumerate() {
        // Distance from this insn to the ERRNO insn = remaining comparisons.
        let jt = n - 1 - i as u8;
        prog.push(BpfInsn { code: BPF_JEQ_K, jt, jf: 0, k: *nr });
    }
    // ERRNO(EPERM) for any matched socket syscall.
    prog.push(BpfInsn { code: BPF_RET_K, jt: 0, jf: 0, k: seccomp_ret_errno(1) });
    // Default: ALLOW.
    prog.push(BpfInsn { code: BPF_RET_K, jt: 0, jf: 0, k: SECCOMP_RET_ALLOW });
    prog
}

/// Hex SHA-256 over the serialized program (each insn as little-endian
/// code|jt|jf|k) — for the receipt's `SeccompRef.hash`.
#[must_use]
pub fn net_deny_bpf_hash(socket_nrs: &[u32], audit_arch: u32) -> String {
    let prog = build_net_deny_bpf(socket_nrs, audit_arch);
    let mut h = Sha256::new();
    for insn in &prog {
        h.update(insn.code.to_le_bytes());
        h.update([insn.jt, insn.jf]);
        h.update(insn.k.to_le_bytes());
    }
    hex::encode(h.finalize())
}
```

In `lib.rs`: `pub mod seccomp;` (host-testable, not android-gated) + `pub use seccomp::{build_net_deny_bpf, net_deny_bpf_hash, net_deny_policy_name, BpfInsn};`.

> Verify the JEQ jump arithmetic by tracing the test program by hand: matched syscall → falls to the ERRNO ret; no match → drops through to ALLOW. If the offset math is fiddly, an equally valid (clearer) shape is "JEQ nr, jt=<to ERRNO via absolute>" using one ERRNO ret per syscall — correctness over compactness; the hash just has to be stable. Whatever shape, the test's invariants (default ALLOW last, exactly EPERM for denies, no KILL except the arch guard) must hold.

- [ ] **Step 4: Run → 3 PASS.**

- [ ] **Step 5: Commit**

```bash
git add platforms/android-minijail/src/seccomp.rs platforms/android-minijail/src/lib.rs platforms/android-minijail/Cargo.toml lingxi-code/Cargo.lock
git commit -m "feat(android-minijail): net-deny seccomp BPF builder + hash (host); raw-filter not policy-file"
```

---

### Task 3: `JailSpec` / `JailedOutput` boundary types (host-testable)

The safe `platform-android` side builds a plain `JailSpec` from the `AndroidSandboxPlan`; the FFI `run_jailed` (Task 4) consumes it. Defining the types + the plan→spec translation first lets the whole translation layer be host-tested before any FFI exists.

**Files:**
- Modify: `platforms/android-minijail/src/lib.rs` (the spec/output types live here, NOT android-gated — they're plain structs)

- [ ] **Step 1: Write the failing test** — in `lib.rs` (host tests module):

```rust
    #[test]
    fn jail_spec_and_output_construct() {
        let spec = JailSpec {
            filename: "/system/bin/sh".into(),
            argv: vec!["sh".into(), "-c".into(), "true".into()],
            envp: vec![("HOME".into(), "/data/x".into())],
            cwd: "/data/x".into(),
            rlimits: vec![JailRlimit { resource: libc_rlimit_cpu(), soft: 30, hard: 30 }],
            net_deny: true,
            bpf: build_net_deny_bpf(&[198, 199], 0xC000_00B7),
            timeout_ms: 120_000,
        };
        assert_eq!(spec.argv.len(), 3);
        let out = JailedOutput {
            stdout: "ok\n".into(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
            enforcement_failed: None,
        };
        assert!(out.enforcement_failed.is_none());
    }
```

- [ ] **Step 2: Run → FAIL** (`cargo test -p platform-android-minijail jail_spec`).

- [ ] **Step 3: Implement** — in `lib.rs`:

```rust
/// An rlimit to apply in the jailed child (resource = a raw `RLIMIT_*` int).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JailRlimit {
    /// Raw `RLIMIT_*` constant (resolved by the caller — `platform-android`
    /// has no libc, so the resource is passed as the integer).
    pub resource: i32,
    /// Soft limit.
    pub soft: u64,
    /// Hard limit.
    pub hard: u64,
}

/// Everything `run_jailed` needs to fork+jail+exec one command. Built by the
/// safe `platform-android` side from an `AndroidSandboxPlan`.
#[derive(Debug, Clone)]
pub struct JailSpec {
    /// Absolute exec target (`/system/bin/sh` for v1).
    pub filename: String,
    /// Full argv (argv[0] included).
    pub argv: Vec<String>,
    /// The complete child environment (scrubbed allowlist — the ONLY env).
    pub envp: Vec<(String, String)>,
    /// Working directory applied via a PRE_EXECVE hook (minijail has no cwd API).
    pub cwd: String,
    /// Rlimits to apply before exec.
    pub rlimits: Vec<JailRlimit>,
    /// Whether to install the net-deny seccomp filter.
    pub net_deny: bool,
    /// The net-deny classic-BPF program (from `build_net_deny_bpf`, empty when
    /// `!net_deny`); `run_jailed` maps it to `sock_fprog` and injects it via
    /// `minijail_set_seccomp_filters`.
    pub bpf: Vec<BpfInsn>,
    /// Wall-clock budget; the watchdog kills the process group past this.
    pub timeout_ms: u64,
}

/// Result of a completed (or timed-out / enforcement-failed) jailed run.
#[derive(Debug, Clone)]
pub struct JailedOutput {
    /// Captured stdout (UTF-8 lossy).
    pub stdout: String,
    /// Captured stderr (UTF-8 lossy).
    pub stderr: String,
    /// Exit code (`-1` if signalled).
    pub exit_code: i32,
    /// True when the watchdog killed the group for exceeding `timeout_ms`.
    pub timed_out: bool,
    /// `Some(reason)` when jail SETUP failed (filter load, rlimit, fork) —
    /// the runner maps this to `SandboxEnforcementFailed`, never a silent run.
    pub enforcement_failed: Option<String>,
}

/// `RLIMIT_CPU` as an `i32` for [`JailRlimit`] — a tiny helper so the host test
/// (and `platform-android`) need not depend on libc just to name the constant.
#[must_use]
pub fn libc_rlimit_cpu() -> i32 {
    0 // RLIMIT_CPU == 0 on Linux/Android.
}
```

(The real `RLIMIT_*` integers are stable Linux ABI; `platform-android` will map `RlimitResource` → these ints directly in Task 5. Keeping the resource an `i32` is what lets `platform-android` stay libc-free.)

- [ ] **Step 4: Run → PASS; Step 5: Commit**

```bash
git add platforms/android-minijail/src/lib.rs
git commit -m "feat(android-minijail): JailSpec/JailedOutput boundary types (host)"
```

---

### Task 4: `run_jailed` — the FFI execution path (android-cfg; device-verified)

The heart of P2. All unsafe. Forks a jailed child, captures pipes, self-enforces the timeout by killing the process group, reaps, returns `JailedOutput`. Host build compiles a stub that returns `enforcement_failed: Some("requires an Android device")`.

**Files:**
- Create: `platforms/android-minijail/src/run.rs`
- Modify: `platforms/android-minijail/src/lib.rs` (`mod run;` + `pub use run::run_jailed;`)

- [ ] **Step 1: Host stub + host test first.** In `run.rs`:

```rust
//! `run_jailed`: fork + jail + exec one command, capture output, enforce the
//! timeout by killing the process group, reap. The only fork+exec path in the
//! engine (spec r3 D6: in-engine `minijail_run_*`). All unsafe FFI is here.

use crate::{JailSpec, JailedOutput};

/// Run `spec` to completion under Minijail. Host builds cannot jail — they
/// report enforcement failure so callers stay fail-closed.
#[must_use]
pub fn run_jailed(spec: &JailSpec) -> JailedOutput {
    #[cfg(not(target_os = "android"))]
    {
        let _ = spec;
        JailedOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: -1,
            timed_out: false,
            enforcement_failed: Some("jailed execution requires an Android device".into()),
        }
    }
    #[cfg(target_os = "android")]
    {
        android_impl::run(spec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_run_reports_enforcement_failure() {
        let spec = JailSpec {
            filename: "/system/bin/sh".into(),
            argv: vec!["sh".into()],
            envp: vec![],
            cwd: "/".into(),
            rlimits: vec![],
            net_deny: false,
            bpf: vec![],
            timeout_ms: 1000,
        };
        let out = run_jailed(&spec);
        assert!(out.enforcement_failed.is_some());
        assert_eq!(out.exit_code, -1);
    }
}
```

- [ ] **Step 2: Run → host test PASS** (`cargo test -p platform-android-minijail run_jailed`).

- [ ] **Step 3: Implement the android module.** Add to `run.rs` an `#[cfg(target_os = "android")] mod android_impl` that:
  1. Extends the FFI block (the P0a `extern "C"` already binds `minijail_new/no_new_privs/preserve_fd/wait/destroy` + the P0a exec call — ADD the entries this needs): `minijail_rlimit(j, type: c_int, cur: u64, max: u64) -> c_int`, `minijail_use_seccomp_filter(j)`, `minijail_set_seccomp_filters(j, filter: *const SockFprog)` (raw-filter injection — NOT the policy-file path; see Task 2 CORRECTION), `minijail_create_session(j) -> c_int`, `minijail_add_hook(j, hook: extern "C" fn(*mut c_void) -> c_int, payload: *mut c_void, event: c_int) -> c_int`, and **switch the exec call to `minijail_run_env_pid_pipes_no_preload`** (the env variant — we pass the scrubbed `envp` explicitly; the no-env P0a variant inherited the engine env, which v1 must NOT do). Define a local `#[repr(C)] struct SockFprog { len: c_ushort, filter: *mut SockFilter }` and `#[repr(C)] struct SockFilter { code: u16, jt: u8, jf: u8, k: u32 }` (or use `libc::sock_filter`/`libc::sock_fprog` if the pinned libc exposes them — check `libc::sock_fprog`). Confirm every signature against `third_party/minijail/libminijail.h` (`run_env_pid_pipes_no_preload(j, filename, argv, envp, *pid, *in, *out, *err)`; `set_seccomp_filters(j, const struct sock_fprog *)`; `MINIJAIL_HOOK_EVENT_PRE_EXECVE` = read the enum's numeric position). **Do NOT bind/call `minijail_log_seccomp_filter_failures` — libminijail.c:1603 `die()`s if it was called together with `set_seccomp_filters`.**
  2. Builds the jail: `minijail_new` → `minijail_no_new_privs` → each `minijail_rlimit` → if `net_deny`: map `spec.bpf` (the `Vec<BpfInsn>` from Task 2, passed in the spec) to a `Vec<SockFilter>`, build a `SockFprog { len, filter: vec.as_mut_ptr() }` (KEEP the Vec alive until after the jail is fully built/run), `minijail_use_seccomp_filter(j)` + optional `minijail_set_seccomp_filter_tsync(j)` (TSYNC applies the filter to all threads — bind it too; it is NOT the log fn and is compatible) + `minijail_set_seccomp_filters(j, &fprog)` → `minijail_create_session` (new session/pgid so the watchdog can `kill(-pgid)`) → register a PRE_EXECVE chdir hook whose payload is the `cwd` CString (the hook calls `chdir(payload)` and returns 0/-errno). The arch guard inside the BPF means a wrong-arch kernel KILLs rather than misfiring — acceptable (the probe already confirmed the arch).
  3. Marshals `argv`/`envp` as NULL-terminated `*mut c_char` arrays (CStrings kept alive across the call, exactly like the P0a smoke).
  4. `minijail_run_env_pid_pipes_no_preload(...)` → child pid + stdout/stderr fd out-params (pass NULL for stdin fd; feed `spec` has no stdin in v1 — note: ProcessCommand.stdin exists but the Shell tool doesn't set it; wire stdin in a later task, leave a `// stdin deferred` marker). On nonzero return → `enforcement_failed`.
  5. Timeout + capture: spawn a watchdog thread that sleeps `timeout_ms` then `kill(-pgid, SIGKILL)` and sets a shared `timed_out` flag; read stdout+stderr fds to EOF on the calling thread (or two reader threads to avoid pipe-buffer deadlock — a 64KB stderr + large stdout will deadlock a single-threaded sequential read; use two threads or poll both); `minijail_wait` → exit status (P0a established its encoding: `n & 0xFF` for exit, signal-base for signals; a SIGKILL from the watchdog reads as timed-out). Cancel the watchdog if the child exits first.
  6. Always `minijail_destroy` (the Drop guard from P0a); close all fds.

  Document each `unsafe` block with a SAFETY comment (the crate is the only one allowed unsafe; reviewers will scrutinize argv/CString lifetimes, fd ownership/double-close, the hook payload outliving the call, and the pgid kill targeting the child's session not the engine's).

- [ ] **Step 4: Cross-compile gate (host cannot run it).**

Run: `export ANDROID_NDK_HOME=~/Library/Android/sdk/ndk/27.0.12077973 && cargo ndk -t arm64-v8a build -p platform-android-minijail && cargo ndk -t arm64-v8a clippy -p platform-android-minijail -- -D warnings && cargo test -p platform-android-minijail run_jailed`
Expected: cross-build + android-clippy clean; host stub test passes.

- [ ] **Step 5: Commit**

```bash
git add platforms/android-minijail/src/run.rs platforms/android-minijail/src/lib.rs
git commit -m "feat(android-minijail): run_jailed — jailed fork/exec, pipe capture, pgid timeout (P2)"
```

---

### Task 5: wire the runner — `AndroidMinijailProcessRunner::run()` executes

**Files:**
- Modify: `platforms/android/src/process.rs`
- Modify: `platforms/android/src/lib.rs` (runner gets the shared cache from Task 1)
- Create: `platforms/android/src/receipt.rs` (+ module decl/re-export)

- [ ] **Step 1: receipt.rs first (host-testable).** Create `receipt.rs`:

```rust
//! Enforcement receipt (spec r3 §AndroidMinijailProcessRunner): what was
//! ACTUALLY applied to a jailed run. Attached to the process log / tool meta.

use crate::policy::{AndroidSandboxPlan, ExecTarget, NetProfile};
use platform_api::SandboxBackend;

/// Honest record of one jailed execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AndroidSandboxReceipt {
    /// Always `AndroidMinijail`.
    pub backend: SandboxBackend,
    /// `"SystemShell"` or the helper name.
    pub target: String,
    /// Helper hash when a bundled helper (P4+); `None` for system sh.
    pub target_hash: Option<String>,
    /// `no_new_privs` was set.
    pub no_new_privs: bool,
    /// Net-deny seccomp filter applied.
    pub net_deny: bool,
    /// The seccomp policy name + hash, when a filter was applied.
    pub seccomp_policy: Option<String>,
    /// Rlimit resources applied (string forms for the log).
    pub rlimits: Vec<String>,
    /// Landlock was NOT applied (always false in v1 — recorded for honesty).
    pub landlock_enforced: bool,
    /// fs confinement reality.
    pub fs_confinement: String,
    /// Features the policy requested but the platform cannot enforce.
    pub unsupported_required_features: Vec<String>,
}

impl AndroidSandboxReceipt {
    /// Build the planned receipt from the prepared plan (pre-execution preview;
    /// the runner stamps execution outcome separately in logs).
    #[must_use]
    pub fn from_plan(plan: &AndroidSandboxPlan) -> Self {
        let (target, target_hash) = match &plan.target {
            ExecTarget::SystemShell => ("SystemShell".to_string(), None),
            ExecTarget::BundledHelper { name, hash, .. } => (name.clone(), Some(hash.clone())),
        };
        Self {
            backend: SandboxBackend::AndroidMinijail,
            target,
            target_hash,
            no_new_privs: true,
            net_deny: plan.network == NetProfile::DenyNet,
            seccomp_policy: plan
                .seccomp_policy
                .as_ref()
                .map(|s| format!("{}@{}", s.name, s.hash)),
            rlimits: plan.rlimits.iter().map(|r| format!("{:?}", r.resource)).collect(),
            landlock_enforced: false,
            fs_confinement: "app_uid_only".to_string(),
            unsupported_required_features: Vec::new(),
        }
    }
}
```

Tests: `from_plan` for a DenyNet SystemShell plan (net_deny true, seccomp_policy Some, fs_confinement "app_uid_only", landlock false) and a bundled-helper AllowNet plan (target_hash Some, net_deny false). Add `pub mod receipt; pub use receipt::AndroidSandboxReceipt;` to lib.rs.

- [ ] **Step 2: failing runner test.** In `process.rs` tests — the host cannot jail, so assert the host path maps enforcement failure to a structured error, and that a DenyNet plan with NO seccomp ref is refused BEFORE spawn:

```rust
    #[tokio::test]
    async fn host_run_maps_enforcement_failure_to_structured_error() {
        // A valid AndroidMinijail plan reaches run_jailed, which on the host
        // reports enforcement failure → SandboxEnforcementFailed (NOT Io/Unsupported).
        let sc = sandboxed_with_plan(deny_net_plan_with_seccomp());
        let err = AndroidMinijailProcessRunner::new(ready_cache())
            .run(&sc)
            .await
            .unwrap_err();
        assert!(matches!(err, ProcessError::SandboxEnforcementFailed(_)));
    }

    #[tokio::test]
    async fn deny_net_plan_without_filter_capability_fails_closed() {
        // caps say seccomp_filter=false → a DenyNet plan must be refused with a
        // named PolicyUnsupported, never run unconfined.
        let caps = /* probed, smoke, nnp true, seccomp_filter:false, net_deny_verified:false */;
        let err = AndroidMinijailProcessRunner::new(caps)
            .run(&sandboxed_with_plan(deny_net_plan_with_seccomp()))
            .await
            .unwrap_err();
        assert!(matches!(err, ProcessError::PolicyUnsupported(ref m) if m.contains("seccomp")));
    }
```

(Add the test helpers: `ready_cache()` = Arc<CapabilityCache> with smoke+nnp+seccomp_filter+net_deny_verified all true; `deny_net_plan_with_seccomp()` = an AndroidSandboxPlan with DenyNet + `SeccompRef`. `sandboxed_with_plan` mirrors the P1 helper that wraps a plan in a `Wrapped{AndroidMinijail}` SandboxedCommand.)

- [ ] **Step 3: implement.** `AndroidMinijailProcessRunner` gains `caps: Arc<CapabilityCache>` + `new(caps)`. Update `lib.rs` Task-1 site to pass `caps.clone()` into the runner. In `run()`, AFTER `admitted_plan` succeeds:
  1. **Per-plan capability gate (spec sandbox.rs P2 NOTE):** if `plan.network == DenyNet` and not (`caps.seccomp_filter && caps.net_deny_verified`) → `Err(PolicyUnsupported("net-deny seccomp filter unavailable; cannot honor deny-net"))`.
  2. Translate `AndroidSandboxPlan` → `JailSpec`: map each `RlimitResource` → the raw `RLIMIT_*` int (`Cpu=0, As=9? — VERIFY the Linux ABI numbers: RLIMIT_CPU=0, RLIMIT_FSIZE=1, RLIMIT_AS=9, RLIMIT_NOFILE=7, RLIMIT_CORE=4`; encode the four the policy uses); `net_deny` = DenyNet; `bpf` = `platform_android_minijail::build_net_deny_bpf(socket_nrs, audit_arch)` when DenyNet else `vec![]` (the runner gets `socket_nrs`/`audit_arch` from a tiny `platform-android-minijail` helper that returns the per-arch `libc::SYS_*`/`AUDIT_ARCH_*` — host-stub returns empties); `cwd` = `inner.cwd` (canonicalized in prepare); `timeout_ms` = `inner.timeout` or the default; `envp`/`argv`/`filename` from plan+inner.
  3. `tokio::task::spawn_blocking(move || platform_android_minijail::run_jailed(&spec)).await` → `JailedOutput`.
  4. `out.enforcement_failed` → `Err(SandboxEnforcementFailed(reason))`. Else `Ok(ProcessOutput { stdout, stderr, exit_code, timed_out })` (mirrors `platforms/posix` contract incl. `timed_out`; the Shell tool's timeout-message path reads it).
  5. Build `AndroidSandboxReceipt::from_plan(plan)` and log it (`tracing::info!`). `is_available()` now returns `self.caps.get().available()` (was hard-false in P1).

> The `RLIMIT_*` translation lives on the `platform-android` side (libc-free): hardcode the stable Linux ABI integers in a small `fn rlimit_resource_int(r: RlimitResource) -> i32` with a comment citing `<asm-generic/resource.h>`, and a host test pinning the four values. This avoids a libc dep in the forbid-unsafe crate.

- [ ] **Step 4: gates.**

Run: `cargo test -p platform-android && export ANDROID_NDK_HOME=~/Library/Android/sdk/ndk/27.0.12077973 && cargo ndk -t arm64-v8a clippy -p platform-android -- -D warnings && cargo clippy -p platform-android --all-targets -- -D warnings && cargo fmt -p platform-android --check && cargo check --workspace`
Expected: all clean.

- [ ] **Step 5: Commit**

```bash
git add platforms/android/src/process.rs platforms/android/src/lib.rs platforms/android/src/receipt.rs
git commit -m "feat(platform-android): runner executes via run_jailed; per-plan net-deny gate; receipt (P2)"
```

---

### Task 6: complete the capability probe (android-cfg; device-verified)

**Files:**
- Create: `platforms/android-minijail/src/probe.rs` (FFI probe extras)
- Modify: `platforms/android-minijail/src/lib.rs` (re-export a `ProbeExtras` + `probe_extras()`)
- Modify: `platforms/android/src/capabilities.rs` (android body fills the remaining fields)

- [ ] **Step 1: host-testable shape.** `probe.rs` exposes `ProbeExtras { seccomp_filter, seccomp_tsync, net_deny_verified, pgid_kill, landlock_abi, system_sh_version, toybox_applets }` and `pub fn probe_extras() -> ProbeExtras`; host build returns all-false/empty. Host test asserts the host stub is conservative.

- [ ] **Step 2: android impl.** `#[cfg(target_os = "android")]`: install a harmless seccomp filter in a throwaway fork (sets `seccomp_filter`); attempt `minijail_set_seccomp_filter_tsync` (`seccomp_tsync`); fork a child under the net-deny filter and have it call `socket(AF_INET, SOCK_STREAM, 0)` expecting EPERM (`net_deny_verified`); fork a child in its own session, `kill(-pgid)`, confirm reaped (`pgid_kill`); read Landlock ABI via `landlock_create_ruleset(NULL, 0, LANDLOCK_CREATE_RULESET_VERSION)` (expected -1/ENOSYS → `None`); run `/system/bin/sh -c 'echo $KSH_VERSION'` for `system_sh_version`; run `toybox` (or `ls /system/bin` filtered) for `toybox_applets`. Each step independently fallible — a failure sets its field false/None, never panics.

- [ ] **Step 3: wire into capabilities.rs.** The android body of `probe_android_capabilities()` (currently just the smoke) calls `probe_extras()` and fills the fields:

```rust
        let smoke = platform_android_minijail::minijail_smoke();
        let extras = platform_android_minijail::probe_extras();
        AndroidSandboxCapabilities {
            probed: true,
            minijail_smoke: smoke.ok,
            no_new_privs: smoke.no_new_privs,
            seccomp_filter: extras.seccomp_filter,
            seccomp_tsync: extras.seccomp_tsync,
            net_deny_verified: extras.net_deny_verified,
            pgid_kill: extras.pgid_kill,
            landlock_abi: extras.landlock_abi,
            system_sh_version: extras.system_sh_version,
            toybox_applets: extras.toybox_applets,
            reason: smoke.reason,
        }
```

- [ ] **Step 4: gates** (host tests + arm64 cross-build + android clippy + workspace check).

- [ ] **Step 5: Commit**

```bash
git add platforms/android-minijail/src/probe.rs platforms/android-minijail/src/lib.rs platforms/android/src/capabilities.rs
git commit -m "feat(android): complete capability probe — seccomp/tsync/net-deny/pgid/landlock/sh/toybox (P2)"
```

---

### Task 7: eager probe wiring in engine-mobile

**Files:**
- Modify: `apps/engine-mobile/src/host.rs` (or wherever `build_mobile_engine` assembles the registry — confirm)
- Modify: `apps/android-aar/src/lib.rs` if the probe must run in `build_android_engine` before registry assembly

- [ ] **Step 1:** Read `build_mobile_engine` + how the platform reaches it. The probe is android-only and the cache lives on `AndroidPlatform` (Task 1). The seam: after the `AndroidPlatform` is constructed but before `register_mobile_tools`, if `platform.shell_capability_cache()` is `Some(cache)` and unprobed, run `block_on(platform_android::capabilities::probe_android_capabilities())` and `cache.set(result)`. Because `build_mobile_engine` is shared (iOS too) and takes `Arc<dyn Platform>`, the cleanest place is the **Android FFI constructor** (`apps/android-aar/src/lib.rs build_android_engine`, android branch) which holds the concrete `AndroidPlatform` and owns a runtime handle — probe there and set the cache before calling `engine_mobile::build_mobile_engine`. Confirm a runtime exists at that point (the handle owns tokio; use `Handle::current()` or the handle's `block_on`).

- [ ] **Step 2:** host test — the off-device shared `build_mobile_engine` test (the `android-aar` HostFakePlatform path) still builds; add an assertion that a fake platform exposing an unprobed cache does not panic and registers no shell tool (registration gates from P3 aren't in yet, so just assert the engine builds). Keep it minimal — this seam is mostly device-validated.

- [ ] **Step 3:** Implement; **Step 4:** `cargo test -p android-aar -p engine-mobile && cargo check --workspace && cargo ndk -t arm64-v8a build -p android-aar`; **Step 5:** commit:

```bash
git commit -m "feat(android-aar): eager capability probe populates the shared cache before registration (P2/D8)"
```

---

### Task 8: on-device instrumentation — run / timeout / net-deny / exit code

**Files:**
- Create: `clients/android/app/src/androidTest/java/com/lingxi/code/SandboxRunTest.kt`
- Modify: `apps/android-aar/src/lib.rs` — add a test-only UniFFI export `android_sandbox_run_probe(command: String) -> String` that builds a deny-net plan via the real `Sandbox::prepare` + runs it via the real runner and returns `{stdout,stderr,exit_code,timed_out,enforcement_failed}` JSON (so the instrumentation test exercises the REAL prepare→run path, not a bypass).

- [ ] **Step 1:** the Rust export — construct an `AndroidMinijailSandbox` + `AndroidMinijailProcessRunner` over a probed cache + a workspace under the app filesDir, `prepare` the command, `run` it, serialize the `ProcessOutput`/error. Host build returns `{"enforcement_failed":"host build"}`.

- [ ] **Step 2:** `SandboxRunTest.kt` (package `com.lingxi.code`, mirror `SandboxSmokeTest`): assert `echo hello` → stdout "hello\n", exit 0, not timed_out; `false` → exit 1; `sleep 5` with a 1s timeout → timed_out true (and the process group is gone — assert via a follow-up `pgrep`-style check is hard on-device; assert `timed_out`); **net-deny**: a command that opens a socket (`toybox nc` or a tiny `ping`-ish) → fails / non-zero (best-effort; the strong proof is the probe's `net_deny_verified`, assert that via `android_sandbox_smoke`-style probe export if cheaper).

- [ ] **Step 3 (device gate):** `bash clients/android/scripts/build-jni.sh && cd clients/android && ./gradlew :app:connectedDebugAndroidTest --tests "*SandboxRunTest*"` on the API-34 arm64 emulator (boot it as in P0a Task 17). **If the emulator is unavailable, report PENDING-DEVICE with the runbook; commit the build artifacts (Steps 1-2) regardless** — the cross-build + host tests are the merge gate, the device run is the acceptance gate.

- [ ] **Step 4:** Commit:

```bash
git commit -m "feat(android-aar): on-device sandbox run/timeout/net-deny instrumentation (P2 acceptance)"
```

---

### Task 9: P2 gate — workspace fmt/clippy/tests + cross-build

- [ ] **Step 1:** `cargo fmt --all` (revert any reformatting outside this branch's crates, as in P1 Task 13). 
- [ ] **Step 2:** `cargo clippy --workspace --all-targets -- -D warnings` (+ `cargo ndk -t arm64-v8a clippy -p platform-android -p platform-android-minijail -- -D warnings` for the android-cfg code). 
- [ ] **Step 3:** `cargo test --workspace` — green (the `cwd_persistence` macOS tempdir flake is known: rerun `-p tool-shell --test cwd_persistence` once if it trips). 
- [ ] **Step 4:** `cargo ndk -t arm64-v8a build -p android-aar && cargo ndk -t x86_64 build -p android-aar` — full cdylib links. 
- [ ] **Step 5:** Commit any fixups.

```bash
git commit -m "chore(android-sandbox): P2 gate — workspace + android-target clean"
```

---

## Self-review / spec coverage

- Spec §AndroidMinijailProcessRunner: jailed fork/exec (T4), no_new_privs+rlimits+net-deny seccomp (T4/T5), `run_*_pid_pipes` for stdio capture (T4), setsid+pgid timeout kill (T4), receipt (T5), `spawn_background`/`kill` stay `Unsupported` (unchanged from P1 — non-goal 5). ✓
- Spec §Policy mapping `network=Disabled` ⇒ socket-family seccomp deny: T2 (BPF builder) + T4 (raw-filter inject) + T5 (per-plan gate). ✓
- Spec §Capability probing items 2-9 (seccomp/tsync/net-deny/pgid/landlock/sh/toybox): T6. ✓
- D8 eager probe before sync registration: T7. ✓
- Fail-closed: DenyNet without filter capability refused (T5 test); enforcement failure → structured error not silent run (T4/T5). ✓
- `platform-android` stays forbid(unsafe): all FFI in T4/T6 lives in `platform-android-minijail`. ✓
- Deferred to P3+ (out of P2 scope, noted): `enable_shell`/D11 registration gates; Shell tool registration; bundled helpers/Git; stdin wiring; physical-device + API-29 runs; third_party/minijail vendoring (infra debt — flag again at P2 merge).

## Risks specific to P2

- **Pipe-buffer deadlock**: single-threaded sequential read of stdout then stderr deadlocks when the child fills the stderr pipe buffer first. T4 must read both concurrently (two threads or poll). Called out in T4 Step 3.
- **Watchdog races the natural exit**: the watchdog thread must be cancellable (the child may exit in 1ms); a stale `kill(-pgid)` after the pgid is reused could kill an unrelated group — guard with the `timed_out` flag set BEFORE the kill and only kill if the child hasn't been reaped.
- **`minijail_wait` after watchdog SIGKILL**: confirm the exit encoding for a signalled child maps to `timed_out`, not a misleading exit code.
- **seccomp model**: minijail policy *files* are allowlist/default-KILL (verified `syscall_filter.c:817`), so net-deny is a hand-built raw BPF (allow-default, socket→EPERM) injected via `minijail_set_seccomp_filters` (NOT a policy file). Per-arch socket nrs + `AUDIT_ARCH` come from `libc::SYS_*` on-device; the BPF builder is host-tested with sample nrs. `set_seccomp_filters` is incompatible with `log_seccomp_filter_failures` (libminijail.c:1603 die) — never call both.
- **Device gate availability**: the emulator is the only verification for the unsafe path; if CI lacks it, the cross-build + host translation tests gate the merge and the device run is a tracked acceptance step.
