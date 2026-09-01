# Android Minijail Sandbox Design

Date: 2026-06-12
Status: SUPERSEDED by `2026-06-12-android-sandbox-shell-design.md` (kept for the minijail analysis trail)

## Summary

Android shell support should be built as an **Android hardened process runtime** backed by Google Minijail, not as a desktop-equivalent sandbox. A normal Android app already runs inside an OS app sandbox, but it cannot reliably create the Linux namespace, chroot, bind-mount, or network namespace isolation used by desktop Linux. Minijail is still useful for additional child-process hardening: `no_new_privs`, seccomp-bpf, rlimits, explicit file descriptor handling, environment cleanup, and optional Landlock filesystem restrictions.

The product path is limited to known bundled helper binaries. Limited `/system/bin/sh` execution is an explicit opt-in diagnostics path. Arbitrary user-provided binaries are unsupported.

The v1 implementation must prove two Android-specific constraints before any runtime work proceeds:

1. the helper executable and libminijail can be built and launched on Android API 29+ through an APK/package-manager-controlled executable location, not from app-writable `filesDir`, `cacheDir`, or extracted assets;
2. the sandbox plan can be carried from `Sandbox::prepare()` to `ProcessRunner::run()` without re-deriving policy in the runner.

## Context

LingXi already has the right execution boundary:

- `platform_api::Sandbox` converts a raw `ProcessCommand` plus `SandboxPolicy` into a `SandboxedCommand`.
- `platform_api::ProcessRunner` accepts only `SandboxedCommand`, so process execution cannot bypass the sandbox decision path.
- `platform-android` currently wires `platform_posix_minimal::PosixProcess` and `PosixSandbox`, both stubs.
- `engine-mobile` currently omits shell tools because process and sandbox support are not implemented.

The type boundary is necessary but not sufficient: `Sandbox::bypass_with_audit()` can also mint a `SandboxedCommand`. Android process execution must therefore reject audited bypass commands and any non-Android backend tag. The first Android helper/shell implementation must call `Sandbox::prepare()` and must not reuse desktop `BashTool`'s audited-bypass execution shape unchanged.

Minijail was cloned to `third_party/minijail` and analyzed with Graphify. The generated graph identified the Android-relevant communities as `BPF Seccomp Core`, `Landlock FS Rules`, `Libminijail API`, `Core Jail Runtime`, `Rust Minijail Wrapper`, and `CLI Jail Options`. The report is in `third_party/minijail/graphify-out/GRAPH_REPORT.md`.

## Key Findings from Minijail

Minijail provides:

- process launching through `minijail_run*` / Rust `Minijail::run*` APIs;
- `no_new_privs`;
- seccomp-bpf policy loading and logging;
- rlimits;
- file descriptor closing and preservation;
- namespace controls for privileged contexts;
- Landlock-backed filesystem restrictions when kernel support is present;
- C API plus Rust wrappers (`minijail`, `minijail-sys`).

Minijail does not turn a normal Android app into a container runtime. User namespaces require PID namespaces in Minijail, and `clone(CLONE_NEWPID | ...)` can fail with `EPERM` / missing `CAP_SYS_ADMIN`. Mount namespaces, bind mounts, chroot, pivot root, network namespaces, cgroups, capability manipulation, and alt-syscall are system-service features, not reliable normal-app features.

Android 10/API 29+ also removes execute permission for app-home files for untrusted apps. The helper cannot be copied into an app-writable private directory and then `execve()`'d. It must be embedded and installed through a package-manager-controlled path that is verified by instrumentation tests. Reference: [Android 10 behavior changes: removed execute permission for app home directory](https://developer.android.com/about/versions/10/behavior-changes-10#execute-permission).

## Goals

1. Add a real Android sandbox/process implementation that fits LingXi's `Sandbox` and `ProcessRunner` traits.
2. Use Minijail for the subset that works in normal Android apps: `no_new_privs`, seccomp filters, rlimits, FD hygiene, env cleanup, and optional Landlock.
3. Fail closed when a required policy cannot be enforced.
4. Expose an honest capability receipt so the engine and UI know what was actually applied.
5. Gate mobile shell/tool registration on runtime capability, not just target OS.

## Non-goals

1. No desktop Linux sandbox parity on Android.
2. No promise of chroot, bind mounts, mount namespace, PID namespace, user namespace, or network namespace for normal apps.
3. No arbitrary downloaded/user-provided binary execution.
4. No hidden downgrade to unsandboxed execution when required policy enforcement fails.
5. No iOS implementation in this spec; iOS will use a separate SwiftBash-based design.

## Architecture

### New modules

Add Android-specific platform modules:

```text
lingxi-code/platforms/android/src/
├── sandbox.rs
├── process.rs
├── capabilities.rs
├── receipt.rs
└── helper_protocol.rs
```

Optional later extraction if the code grows beyond Android platform concerns:

```text
lingxi-code/sandbox-android-runtime/
```

Start inside `platforms/android` to keep the first implementation close to the Android composition root. Extract a crate only when another package needs the runtime independently.

### Android packaging and build constraints

Phase 0 must prove packaging and build feasibility before runtime integration:

- build libminijail for the supported Android ABIs with the Android NDK;
- avoid relying on host `pkg-config` for Android targets;
- decide whether `minijail-sys` uses pre-generated bindings or NDK-aware bindgen arguments;
- package the helper through APK/native packaging so it is executable from a package-manager-controlled location;
- reject helper locations under app-writable `filesDir`, `cacheDir`, or extracted assets;
- run an API 29 instrumentation test that launches the helper and reports its executable path.

If any of these fail, the implementation must stop at capability reporting and leave Android process execution disabled.

### Android platform inputs

`AndroidPlatformInputs` must grow before process execution is enabled:

```rust
pub struct AndroidSandboxConfig {
    pub helper_exec_path: PathBuf,
    pub seccomp_policy_dir: PathBuf,
    pub native_library_dir: PathBuf,
    pub package_source_dirs: Vec<PathBuf>,
    pub app_writable_roots: Vec<PathBuf>,
    pub package_name: String,
    pub package_version_code: i64,
    pub enable_tier1_helpers: bool,
    pub enable_tier2_diagnostics_shell: bool,
}
```

The Kotlin/FFI bootstrap must provide authoritative package metadata from Android, including `ApplicationInfo.nativeLibraryDir`, relevant source/split-source directories, every app-writable root that must be rejected (`filesDir`, `cacheDir`, `codeCacheDir`, `noBackupFilesDir`, and extracted-assets roots), package name, and package version. The Android platform constructor must canonicalize `helper_exec_path`, reject symlink escapes, reject every configured app-writable root, and accept helpers only under a verified package-manager-owned executable directory. It should reject shell support when the config is missing, even if the app was built with Android target support.

### Core types

Add an Android backend identity:

```rust
SandboxBackend::AndroidMinijail
```

Add an Android capability model:

```rust
pub struct AndroidSandboxCapabilities {
    pub helper_spawn: bool,
    pub helper_exec_path: Option<String>,
    pub no_new_privs: bool,
    pub seccomp_filter: bool,
    pub seccomp_tsync: bool,
    pub rlimits: bool,
    pub landlock: bool,
    pub fd_remap: bool,
    pub shell_available: bool,
}
```

Add an Android execution tier and prepared sandbox plan:

```rust
pub enum AndroidExecutionTier {
    BundledHelper,
    DiagnosticSystemShell,
}

pub struct AndroidSandboxPlan {
    pub tier: AndroidExecutionTier,
    pub helper_exec_path: String,
    pub target: AndroidTarget,
    pub seccomp_policy: Option<String>,
    pub seccomp_policy_hash: Option<String>,
    pub rlimits: Vec<String>,
    pub fd_remaps: Vec<FdRemap>,
    pub env_allowlist: Vec<String>,
    pub landlock_required: bool,
    pub process_cleanup: ProcessCleanupStrategy,
}
```

`Sandbox::prepare()` must attach this plan, or an opaque backend plan handle resolving to it, to the returned `SandboxedCommand`. The runner must reject commands tagged with `AndroidMinijail` when the Android plan is missing or malformed. If the current trait model cannot carry backend-specific metadata, Phase 1 must extend `SandboxedTag::Wrapped` before process execution is implemented.

The Android runner must execute only commands whose tag is an Android prepared plan. It must reject `SandboxedTag::BypassAuditedWithReason` and all non-`AndroidMinijail` backends. This is a security invariant, not an implementation detail.

Add an enforcement receipt. The receipt should be attached to Android-specific process execution logs and may later be surfaced in tool metadata:

```rust
pub struct AndroidSandboxReceipt {
    pub backend: SandboxBackend,
    pub tier: AndroidExecutionTier,
    pub helper_exec_path: String,
    pub seccomp_policy: Option<String>,
    pub seccomp_policy_hash: Option<String>,
    pub no_new_privs: bool,
    pub rlimits: Vec<String>,
    pub landlock_available: bool,
    pub landlock_enforced: bool,
    pub env_cleared: bool,
    pub stdio_remapped: bool,
    pub process_cleanup: ProcessCleanupStrategy,
    pub helper_protocol_version: u32,
    pub unsupported_required_features: Vec<String>,
}
```

### Runtime components

#### `AndroidMinijailSandbox`

Implements `platform_api::Sandbox`.

Responsibilities:

1. Probe capabilities lazily and cache them for the session.
2. Validate the command shape and working directory.
3. Translate `SandboxPolicy` into an Android enforcement plan.
4. Fail if the requested policy needs unavailable enforcement.
5. Construct a `SandboxedCommand` tagged with `AndroidMinijail` and carrying the Android plan.
6. Produce a planned receipt preview for audit; the final receipt is emitted by the helper/runner after execution.

`prepare()` must not perform process spawning. It only validates and produces the plan/provenance.

#### `AndroidMinijailProcessRunner`

Implements `platform_api::ProcessRunner`.

Responsibilities:

1. Launch the package-manager-installed native helper rather than forking directly from the main Rust/Kotlin app runtime.
2. Create stdout/stderr pipes or output files.
3. Pass/remap FDs into Minijail with `run_remap()` or equivalent C API because Minijail's Rust `run()` maps stdio to `/dev/null` unless FDs are preserved.
4. Apply `no_new_privs`, rlimits, seccomp filters, and optional Landlock in the helper path before exec.
5. Capture stdout/stderr and exit code into `ProcessOutput`.
6. Enforce timeouts and kill/reap processes using the configured process cleanup strategy.
7. Support background tasks only after foreground execution is reliable.

#### Native helper

The native helper is a small bundled executable installed through the app package. It must not be executed from app-writable storage.

Responsibilities:

1. Receive a serialized helper request through an inherited FD using a framed, versioned protocol.
2. Clear inherited environment except explicit allowlisted variables.
3. Close all non-preserved FDs.
4. Set `CLOEXEC` where appropriate.
5. Configure Minijail.
6. Exec the target known binary or limited shell command.

The helper keeps Minijail setup outside the multi-threaded app runtime and avoids applying irreversible restrictions to the engine process.

The v1 protocol must not use argv for the request body. Argv is limited to the helper path, mode, and request FD number. The framed request has a maximum byte size, a protocol version, and strict unknown-field rejection.

## Execution Tiers

### Tier 1: Known bundled helpers

Default product path.

Characteristics:

- executable is shipped with the app and launched from a package-manager-controlled executable location, or is part of a trusted Android system surface;
- seccomp policy is known and bundled;
- rlimits are deterministic;
- environment is fully controlled;
- FD set is explicit;
- Landlock is used when available.

This tier is compatible with Minijail's known-binaries threat model.

### Tier 2: Limited `/system/bin/sh`

Explicit opt-in diagnostics path.

Characteristics:

- `/system/bin/sh` availability is probed;
- `PATH` is cleared or set to a restricted value;
- commands are not advertised as desktop Bash parity;
- network and subprocess policies are best effort;
- process cleanup uses process-group/session handling where supported;
- failures must state which policy requirement could not be enforced.

This tier should not be the default AI coding shell.

### Tier 3: Arbitrary binaries

Unsupported.

Reasons:

- Minijail is not designed to safely run attacker-controlled binaries;
- Android app data execution restrictions make downloaded executable behavior unreliable;
- without UID/mount namespace isolation, arbitrary binaries inherit too much app authority.

## Policy Mapping

| `SandboxPolicy` field | Android mapping |
|---|---|
| `limits.max_cpu_seconds` | `RLIMIT_CPU` where supported; timeout remains enforced by runner |
| `limits.max_memory_mb` | `RLIMIT_AS` best effort; memory enforcement is device-dependent |
| `limits.max_processes` | Prefer seccomp deny for `fork`/`vfork`/selected `clone`; `RLIMIT_NPROC` is UID-scoped and must be used carefully |
| `limits.max_open_files` | `RLIMIT_NOFILE` |
| `network = Disabled` | seccomp deny selected network syscalls where policy allows; do not claim namespace isolation |
| `network = LoopbackOnly` | unsupported as a strict guarantee in normal apps; fail if required |
| `network = Allowed` | no extra network restriction |
| `writable_paths` | validate inside app root; enforce with Landlock when available |
| `denied_paths` | enforce only with Landlock deny-by-default; otherwise fail if strict denial is required |
| `allow_subprocess = false` | seccomp deny process creation syscalls for known helper binaries |

Filesystem restrictions must be implemented through Landlock or an equivalent FD/file-broker model. Path validation alone is an input validation step, not a sandbox guarantee. Any policy that requests nontrivial filesystem confinement (`writable_paths`, `denied_paths`, or read-only filesystem expectations) must fail closed when neither Landlock nor a broker is available, unless the caller explicitly requested no filesystem confinement.

For timeout cleanup, Tier 1 helpers should avoid subprocess creation by policy. Tier 2 shell execution must either run in a killable process group/session or report subprocess cleanup as unsupported.

## Capability Probing

`probe_capability()` must test real runtime behavior, not infer from API level alone.

Probe:

1. helper executable can be launched from the package-manager-controlled path;
2. `no_new_privs` can be set;
3. a harmless seccomp filter can be installed in a disposable child;
4. seccomp TSYNC support if requested;
5. Landlock ABI and ruleset enforcement;
6. stdout/stderr FD remapping;
7. timeout and kill behavior;
8. `/system/bin/sh` availability for Tier 2 only.

Capability probing should run once during Android platform initialization and cache a conservative result. Tool registration and later execution should read the cached result instead of performing async probing inline from synchronous registration code. Cache invalidation is only needed when the app package version or helper path changes.

Report features conservatively:

```rust
SandboxFeatures {
    network_isolation: false,
    fs_readonly: false,
    fs_readwrite_paths: landlock_available,
    process_limit: seccomp_filter || rlimits,
    no_new_privileges: no_new_privs,
}
```

`fs_readonly` remains `false` because the trait currently means mount-style read-only root semantics. Landlock availability is weaker and should be represented by Android-specific capability fields, not by overstating the cross-platform `SandboxFeatures` flags.

## Tool Registration

`engine-mobile` must not register shell tools merely because `target_os = "android"`.

Registration requires:

1. `platform.process().is_available() == true`;
2. cached Android sandbox capability state is available and successful;
3. capability receipt supports the selected tier;
4. user or build configuration enables Android shell support.

The cached capability state must be available before this synchronous registration path runs. Use one of these implementation shapes:

1. `AndroidPlatform::probe_and_new(...).await` builds the platform only after probing succeeds or records a disabled capability state; or
2. Kotlin/FFI bootstrap performs the asynchronous probe and passes a verified `AndroidSandboxCapabilities` cache into `AndroidPlatformInputs`.

The implementation must not block inside synchronous tool registration and must not register tools before capability state is known.

Initial registration should enable only a constrained Android shell/helper tool, not the desktop `BashTool` unchanged. Reusing `BashTool` can be considered later after Tier 2 semantics are explicitly accepted.

## Failure Semantics

Fail closed when:

- requested network isolation cannot be enforced;
- requested filesystem denial cannot be enforced;
- required seccomp policy cannot be loaded;
- helper binary is missing or not executable;
- helper binary is located under app-writable storage;
- helper protocol version is unsupported or the request contains unknown fields;
- stdio cannot be remapped;
- timeout/kill cannot be enforced;
- policy requests unsupported namespace behavior.

Return errors that name the unsupported guarantee rather than generic `Unsupported` when possible.

The current `ProcessError` variants (`Unsupported`, `Io(String)`, `Timeout`) are too coarse for Android policy diagnostics. Phase 1 must extend the process error surface before Android execution is enabled, for example:

```rust
pub enum ProcessError {
    Unsupported,
    PolicyUnsupported(String),
    MalformedSandboxPlan(String),
    SandboxEnforcementFailed(String),
    Io(String),
    Timeout,
}
```

Android runner rejection paths must use these structured variants rather than collapsing policy failures into `Io`.

## Testing Strategy

### Host tests

Use unit tests for:

- policy-to-plan mapping;
- path canonicalization and symlink rejection;
- receipt construction;
- unsupported policy failure;
- command tier classification;
- helper protocol serialization;
- `SandboxedCommand` plan attachment and runner rejection when the plan is missing.

### Android build and packaging tests

Run before runtime tests:

1. build libminijail for each supported Android ABI;
2. build the helper for each supported Android ABI;
3. package the helper into the APK;
4. assert the helper path is not under app-writable storage;
5. launch the helper on API 29 and current stable Android API.

### Android instrumentation/device tests

Run on at least:

- API 29 baseline;
- current stable Android API;
- one emulator;
- one physical device.

Test:

1. capability probe output;
2. known helper success path;
3. stdout/stderr capture;
4. timeout and kill;
5. timeout cleanup for a shell command that attempts to leave a child process running;
6. seccomp violation behavior;
7. rlimit enforcement;
8. Landlock available/unavailable branches;
9. Tier 2 shell disabled by default;
10. strict policy failure when unsupported.

### Regression tests

Mirror existing process contract expectations where possible:

- foreground command returns `ProcessOutput`;
- non-zero exit is captured, not converted to transport failure;
- timeout returns `ProcessError::Timeout`;
- background behavior is omitted until explicitly implemented.

## Risks

1. **False security claims**: Avoid saying Android supports desktop-equivalent sandboxing.
2. **Filesystem TOCTOU**: Path validation alone is weaker than Landlock or mount namespaces. Prefer FD-based access when possible.
3. **Network policy gaps**: Seccomp can block broad syscall classes, but cannot express loopback-only or domain policy.
4. **Grandchild cleanup**: Without PID namespaces/cgroups, shell grandchildren may survive unless process-group handling is careful.
5. **Packaging complexity**: Bundled helper binaries must be installed through APK/package-manager-controlled executable locations compatible with Android 10+ restrictions. App-writable locations are not acceptable.
6. **Minijail stdio defaults**: forgetting FD remap causes empty output because default stdio is `/dev/null`.
7. **Device fragmentation**: seccomp/Landlock behavior varies; probes and tests must be real-device based.

## Rollout Plan

### Phase 0: Packaging and minijail build proof

- Build libminijail with the Android NDK for supported ABIs.
- Decide generated bindings vs NDK-aware bindgen.
- Build and package the native helper.
- Launch the helper on API 29 and current stable Android API.
- Record the helper executable path in capability output.
- Keep all process execution disabled if this phase fails.

### Phase 1: Capability and trait handoff skeleton

- Add Android modules.
- Implement capability probe and receipt types.
- Extend `SandboxedCommand`/`SandboxedTag` or equivalent backend metadata so Android plans can move from `prepare()` to the runner.
- Add Android platform inputs for helper path, seccomp policy bundle, and build/user feature flags.
- Add authoritative package metadata and app-writable root inputs for helper path validation.
- Define the async/precomputed capability initialization seam.
- Extend `ProcessError` with structured Android policy/enforcement failures.
- Keep process execution disabled.
- Add host tests for policy mapping.
- Add host tests for missing-plan rejection.

### Phase 2: Known helper foreground execution

- Build native helper.
- Run one bundled helper command.
- Capture stdout/stderr.
- Enforce rlimits and `no_new_privs`.
- Send helper requests only through the inherited-FD protocol.
- Add Android instrumentation tests.

### Phase 3: Seccomp policy enforcement

- Bundle a minimal seccomp policy.
- Load policy with Minijail.
- Add violation tests.
- Add policy hash to receipt.

### Phase 4: Optional Landlock

- Probe Landlock.
- Add explicit C helper or Rust/`minijail-sys` wrapper calls for Landlock.
- Enforce app-root allowlist when available.
- Fail strict filesystem-denial policies when unavailable.

### Phase 5: Limited shell opt-in

- Add explicit configuration for Tier 2.
- Probe `/system/bin/sh`.
- Restrict env and shell argv.
- Implement and test process-group/session cleanup or report subprocess cleanup unsupported.
- Document non-parity behavior.

## Open Decisions

1. Whether to expose Android shell support as a new mobile-specific tool or adapt `BashTool` behind capability gates.
2. Whether Landlock absence should disable shell entirely or only strict filesystem policies.
3. Whether Tier 2 shell should ever be enabled outside internal diagnostics builds.

## Recommendation

Proceed with Android Minijail support as a hardened process runtime for known helper binaries. Keep desktop sandboxing untouched. Keep iOS separate. Gate shell exposure until capability probing, helper execution, FD remapping, timeout/kill, and seccomp enforcement all pass on real Android devices.
