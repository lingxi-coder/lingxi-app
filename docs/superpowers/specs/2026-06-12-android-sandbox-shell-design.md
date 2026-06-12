# Android Sandbox & Shell Design

Date: 2026-06-12
Status: Revised design r3 (external review merged + D6 re-decided)
Supersedes: `2026-06-12-android-minijail-sandbox-design.md`

## Summary

Android gets a real shell tool through a **phased, fail-closed hardened process
runtime**: v1 executes the device's own `/system/bin/sh` (mksh) and system
toybox utilities under Minijail restrictions applied **in-engine** via the
`minijail_run_pid_pipes` family — no bundled trampoline executable, no FD
framing protocol. libminijail is linked into the engine `.so` as a *library*,
which is exempt from Android 10 W^X exec restrictions, so **v1 carries zero
exec-packaging risk**.

The model-facing tool is a new mobile-only `Shell` tool (mksh dialect declared
in its prompt, probed toybox applet inventory embedded). Desktop `BashTool` is
untouched. **Shell commands always run deny-net.** Networked git is
deliberately *not* a network mode of the shell: it arrives later as a separate
structured mobile `Git` tool with deterministic argv, so approving `git fetch`
never grants arbitrary `sh -c` network access. A later phase swaps in a
bundled mksh + toybox to lock the interpreter version and dialect across
devices.

Shell enablement is additionally gated on mobile secret storage: today the
mobile OAuth credentials are plaintext files under the app UID
(`PlainTextSecureStorage`), and a no-FS-confinement shell could read them.

## Revision history

| Rev | What changed | Decided by |
|---|---|---|
| r1 | Brainstormed design: in-engine `minijail_run`, per-helper network gating (git net subcommands ask-once → whole `sh -c` tree allowed) | User-approved sections, 2026-06-12 |
| r2 | External (Codex) review: Shell made deny-net-only + structured `Git` tool (D10), secret-storage gate added, `minijail_run_pid_pipes` precision, session-only `AllowAlways` fact; **also reverted D6 to a packaged `sandbox-runner` trampoline + FD protocol** | Codex proposal |
| r3 | Second review adjudicated r2: deny-net Shell / Git tool / secrets gate / precision **kept**; trampoline reversal **rejected** — D6 restored to in-engine (`AndroidHelperLauncher` dropped as over-engineering); internal contradictions fixed (plan serialization, spawn story, `ExecTarget` generality, decision-log traceability) | User decision "B", 2026-06-12 |

## Decision log

| # | Decision | Choice | Rev |
|---|---|---|---|
| D1 | Output scope | Revise + complete the minijail spec into one sandbox + shell spec | r1 |
| D2 | Product target | Repo coding workflow: toybox base set + git, phased | r1 |
| D3 | Tool input form | Real shell interpreter, command-string input | r1 |
| D4 | Permission model | Reuse desktop allowlist/ask rules for shell; network grants exist only on structured non-shell tools | r1, narrowed r2 |
| D5 | Interpreter strategy | Phased: v1 system sh + system toybox → git bundled → bundled mksh/toybox version-lock; system sh demoted to internal diagnostic fallback afterward | r1 |
| D6 | Exec architecture | **In-engine `minijail_run_pid_pipes`** inside `spawn_blocking`; no trampoline executable, no FD protocol (r2's trampoline reversal rejected in r3) | r1, reaffirmed r3 |
| D7 | Plan carriage | New private `plan: Option<BackendPlanHandle>` field on `SandboxedCommand`, NOT a `SandboxedTag` variant change; plan stays in-process, never serialized | r1 |
| D8 | Probe timing | Eager: `block_on(probe())` inside `build_mobile_engine` before synchronous tool registration | r1 |
| D9 | Tool name | `Shell` (not `Bash`) — dialect honesty over habit transfer | r1 |
| D10 | Network granularity | Shell is deny-net only; networked git is a separate structured tool with deterministic argv, never arbitrary `sh -c` | r2 (replaces r1 ask-once tree grant — that leaked the grant to the whole pipeline) |
| D11 | Secrets gate | Shell registration additionally gated on secret storage: Keystore-backed storage flag from the host, or explicit user data-exposure acceptance | r2 |

## Context (verified against the codebase)

- `traits::Sandbox` / `traits::ProcessRunner` boundary as described in the
  superseded spec; `SandboxedCommand` is the only runner input
  (`traits/src/sandbox.rs:179-232`).
- `SandboxedTag::Wrapped` carries only `backend` — no plan payload
  (`traits/src/sandbox.rs:193-205`). Hence D7.
- Desktop `BashTool` never calls `Sandbox::prepare()`: it wraps the command
  string via `ctx.sandbox_runner.wrap(...)` then always mints the command with
  `bypass_with_audit(pcmd, "bash_tool_call")`
  (`tools/shell/src/bash.rs:566-608, 625, 697`). The Android runner rejects
  bypass-tagged commands, so reusing `BashTool` unchanged is structurally
  impossible — a new tool is required, not merely preferred.
- `platform-android` wires posix-minimal stubs (`PosixProcess`, `PosixSandbox`,
  both hard-disabled); `engine-mobile` omits shell tools and its
  `register_mobile_tools` is synchronous (`apps/engine-mobile/src/lib.rs:101`).
- `ProcessError` is `{Unsupported, Io(String), Timeout}` — too coarse for
  policy diagnostics (`traits/src/process.rs:76`).
- **Mobile OAuth credentials are plaintext**: the mobile host wires
  `PlainTextSecureStorage` into the credential manager
  (`apps/engine-mobile/src/host.rs:287`). Load-bearing for D11.
- **`AllowAlways` is session-only on mobile**: the mobile permission gate has
  no `.with_persist` — a device session has no project
  `.claude/settings.local.json` to write back to
  (`apps/engine-mobile/src/host.rs:342-344`).

Android platform facts the design rests on:

- **W^X applies to `execve`, not to library loading.** An APK-shipped `.so`
  can be linked/loaded into the engine regardless of `extractNativeLibs`;
  only on-disk *executables* need the package-manager-extracted
  `nativeLibraryDir` path. This is what keeps libminijail out of the packaging
  problem entirely and confines exec-packaging risk to the bundled git binary
  (P4) and the bundled interpreter (P5).
- `/system/bin/sh` is **mksh**; system utilities are **toybox** (Android 6+).
  Applet inventory drifts by OS version/OEM (e.g. no `awk` before Android 15).
- **Landlock is effectively absent** from shipping Android kernels (GKI does
  not enable it). v1 therefore offers no filesystem confinement; the app's
  own private-directory sandbox is the filesystem boundary.
- **seccomp cannot dereference path arguments**, so `execve` cannot be
  filtered by target path: any real interpreter can exec `/system/bin/*`.
  The "known binaries" threat model of the superseded spec is abandoned
  deliberately for shell children (D3); structured helpers (Git) retain
  binary-identity checks (path + hash).
- **Phantom process killing** (Android 12+): the OS caps app child processes
  (32 system-wide default) and kills them under memory pressure or excessive
  CPU while cached. Affects every child this design spawns.

## Threat model

This runtime constrains **what the executed command can do** — resource
ceilings, `no_new_privs`, network denial, environment hygiene, lifetime
control — protecting against a misbehaving or model-misdriven tool process.
It does **not** protect against compromised app code: every child shares the
app UID and the app's own authority.

Because v1 has no filesystem confinement, the shell can read any plaintext
app-private file reachable by that UID, not only the displayed workspace —
including today's `PlainTextSecureStorage` OAuth tokens. Shell support must
therefore stay disabled until mobile credentials and other secrets are outside
shell-readable plaintext storage (Android Keystore-backed), or the user has
explicitly accepted the exposure (D11).

True privilege separation (`isolatedProcess`) was considered and rejected for
v1: it cannot exec PM-installed binaries, has no usable filesystem access, and
forces a Binder-brokered I/O model; it remains a backlog exploration.

## Goals

1. A real `Sandbox`/`ProcessRunner` implementation for Android behind the
   existing trait seam, with `Sandbox::prepare()` as the only admission path.
2. v1 shell capability with **zero exec-packaging risk** (system sh + system
   toybox; libminijail is a library). The only W^X packaging proofs in the
   whole plan are the git binary (P0b, at P4) and the bundled interpreter (P5).
3. Fail closed: any requested-but-unenforceable policy is rejected at
   `prepare()` with a named guarantee; no silent downgrade, ever.
4. Honest receipts: every execution records what was actually enforced.
5. Capability-gated tool registration (not `target_os` checks).

## Non-goals

1. No desktop-parity sandbox claims on Android (no namespaces, no chroot,
   no mount control, no network namespace).
2. No filesystem confinement in v1 (no Landlock on shipping kernels; path
   validation is input hygiene, not a boundary).
3. No binary-identity guarantees for shell child processes after
   `/system/bin/sh` starts; structured helpers such as Git retain
   binary-identity checks.
4. No arbitrary user-downloaded binary execution.
5. No background shell tasks in v1 (needs phantom-process semantics first).
6. No iOS (separate design).

## Architecture

```text
┌─ Engine process (Rust .so inside the Android app) ──────────────┐
│  ShellTool (tools/shell-mobile)        GitTool (tools/git-mobile,│
│    │ permission rules (desktop          P4+, structured argv,    │
│    │ engine); deny-net policy           per-operation net grant) │
│    ▼                                       │                     │
│  AndroidMinijailSandbox::prepare(cmd, &policy)   [sync]          │
│    ▼  SandboxedCommand{ Wrapped{AndroidMinijail}, plan }         │
│  AndroidMinijailProcessRunner::run                               │
│    │  spawn_blocking → minijail_run_pid_pipes()                  │
│    │    ├ fork ───────────────────────────────┐                  │
└────┼──────────────────────────────────────────┼──────────────────┘
     │                                          ▼ (child)
     │                        no_new_privs + rlimits (+ net-deny
     │                        seccomp unless plan allows net)
     │                        → setsid → execve(
     │                            /system/bin/sh -c …      [v1]
     │                          | nativeLibraryDir/git …   [P4 Git tool])
     └── pid + stdout/stderr pipes returned to the engine
```

libminijail (+ a static libcap) is built with the NDK and linked into the
engine `.so` via `minijail-sys` with pre-generated bindings. Library loading
is exempt from W^X, so **P0a (the global gate) is a build/link/smoke proof,
not a packaging proof**. The Android impls are `cfg(target_os = "android")`
gated; host builds keep the existing stubs so the crate still compiles and
tests on macOS/Linux.

Fork discipline: the engine *does* fork (any child spawn does), but all
Minijail jail setup between `fork` and `execve` is libminijail's
async-signal-safe child path — its designed competence, the standard usage in
multithreaded ChromeOS daemons. The runner calls it from `spawn_blocking`
only; residual risk is covered by device soak tests, and a packaged trampoline
remains a documented fallback shape if real-device soak ever disproves this
(see Revision history — it was evaluated and rejected as the default).

### Module layout

```text
lingxi-code/platforms/android/src/
├── sandbox.rs        AndroidMinijailSandbox (traits::Sandbox)
├── process.rs        AndroidMinijailProcessRunner (traits::ProcessRunner)
├── capabilities.rs   one-shot probe + session cache
├── policy.rs         SandboxPolicy → AndroidSandboxPlan mapping + profiles
└── receipt.rs        AndroidSandboxReceipt

lingxi-code/tools/shell-mobile/   ShellTool (new crate; desktop BashTool untouched)
lingxi-code/tools/git-mobile/     GitTool (P4+; structured networked git)
```

No `helper_protocol.rs`, no `sandbox-runner` crate: there is no trampoline
(D6, r3).

## Trait extensions (minimal churn)

```rust
// traits/src/sandbox.rs
pub struct SandboxedCommand {
    inner: ProcessCommand,
    tag: SandboxedTag,
    plan: Option<BackendPlanHandle>,      // NEW, defaults to None
}
/// Opaque, backend-owned prepared plan. Debug prints a placeholder.
pub struct BackendPlanHandle(Arc<dyn Any + Send + Sync>);

// __new_sandboxed keeps its signature (plan = None) — zero desktop churn.
// NEW: __new_sandboxed_with_plan(inner, tag, plan) for backends that carry one.

pub enum SandboxBackend { …, AndroidMinijail }   // NEW variant

// traits/src/process.rs
pub enum ProcessError {
    Unsupported,
    PolicyUnsupported(String),         // NEW: names the unenforceable guarantee
    MalformedSandboxPlan(String),      // NEW: runner-side plan rejection
    SandboxEnforcementFailed(String),  // NEW: jail setup failed at runtime
    Io(String),
    Timeout,
}
```

`SandboxedTag` itself is unchanged (D7): adding a field to `Wrapped` would
break every existing constructor/match site across posix/windows/tests for
no benefit — the plan rides next to the tag, not inside it. The plan is
**in-process only and never serialized** — true again now that there is no
helper protocol (r3).

### AndroidSandboxPlan

```rust
pub enum ExecTarget {
    /// /system/bin/sh -c <command>  (v1; internal diagnostic fallback after P5)
    SystemShell,
    /// A packaged executable under nativeLibraryDir (git at P4,
    /// bundled mksh/toybox at P5). Identity-checked: canonical path + hash.
    BundledHelper { name: String, path: PathBuf, hash: String },
}

pub struct AndroidSandboxPlan {
    pub target: ExecTarget,
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,    // post-scrub allowlist result
    pub network: NetProfile,           // Shell: DenyNet always; GitTool may AllowNet
    pub rlimits: Vec<Rlimit>,
    pub seccomp_policy: Option<SeccompRef>,  // name + hash; net-deny filter in v1
    pub cleanup: ProcessCleanup,       // KillProcessGroup
}
```

## AndroidMinijailSandbox — `prepare()`

Synchronous; validates and plans, never spawns.

1. Canonicalize `cwd`, reject symlink escapes out of the app workspace
   (existing `SandboxError::PathCanonicalize` / `SymlinkEscape` variants).
2. Map `SandboxPolicy` → plan (table below); **fail closed** with named
   guarantees.
3. Read the capability cache (never probe inline). Minijail unavailable ⇒
   `SandboxError::Unavailable` — execution never silently degrades to
   unsandboxed.

### Policy mapping

| `SandboxPolicy` field | Android v1 mapping |
|---|---|
| `network = Disabled` | seccomp deny of socket-family syscalls in the child (whole process tree) |
| `network = Allowed` | allowed only for `ExecTarget::BundledHelper` plans built by structured tools (Git); `SystemShell` fails with `PolicyUnsupported("networked shell is not supported")` |
| `network = LoopbackOnly` | `Unavailable("loopback-only network policy is unenforceable on Android")` |
| `writable_paths` / `denied_paths` non-empty | `Unavailable("filesystem confinement requires Landlock; not available")` — the default mobile policy requests none |
| `allow_subprocess = false` | `PolicyUnsupported` — both sh and git require children |
| `limits.max_cpu_seconds` | `RLIMIT_CPU` (wall-clock timeout stays authoritative in the runner) |
| `limits.max_memory_mb` | `RLIMIT_AS`, best effort (device-dependent; receipt says so) |
| `limits.max_open_files` | `RLIMIT_NOFILE` |
| `limits.max_processes` | not mapped in v1 — `RLIMIT_NPROC` is UID-scoped and would count the whole app |
| (always) | `RLIMIT_CORE = 0`, `no_new_privs` |

The **default mobile shell policy** is: no filesystem confinement, network
disabled, subprocesses allowed, default limits. Filesystem reality: the app
UID is the boundary, not the displayed workspace; the receipt records
`fs_confinement: app_uid_only`.

## AndroidMinijailProcessRunner

Security invariants, checked before anything runs:

1. **Reject `SandboxedTag::BypassAuditedWithReason`** — the desktop
   `BashTool` shape dies here by construction.
2. Reject `Wrapped` whose backend ≠ `AndroidMinijail`.
3. Reject missing plan / failed downcast ⇒ `MalformedSandboxPlan`.

Execution:

- From a `spawn_blocking` closure, call `minijail_run_pid_pipes()` (or the
  equivalent `minijail-sys` API that returns the child pid plus stdio pipe
  FDs). Plain `minijail_run()` is insufficient: the runner contract requires
  stdout/stderr capture, timeout cleanup, and a reliable child pid. Minijail's
  default stdio is `/dev/null` — the pipes variant is mandatory, not optional.
- Jail config compiled in the parent before fork (policy → minijail object);
  the post-fork child path applies `no_new_privs` → rlimits → optional
  net-deny seccomp → `setsid()` → `execve`.
- Timeout: wall-clock timer in the runner; on expiry `kill(-pgid, SIGKILL)`
  for the whole group, then reap. Output semantics mirror the
  `platforms/posix` runner contract exactly (including the `timed_out` flag
  the desktop BashTool consumes at `tools/shell/src/bash.rs:706`); non-zero
  exit is captured output, never a transport error.
- Emits an `AndroidSandboxReceipt` per run: backend, target (+hash for
  bundled), enforced set, `unsupported_required_features`, network profile,
  rlimits, cleanup strategy, policy hash. Logged and attached to tool result
  metadata.
- `spawn_background` / `kill`: deferred (non-goal 5); the trait methods
  return `Unsupported` in v1.

### Environment (scrub + allowlist rebuild)

| Variable | Value |
|---|---|
| `HOME` | configured shell workspace root (NOT the app files root — keeps `~` away from `.claude/`; hygiene only, not a boundary) |
| `TMPDIR` | app cache dir |
| `PATH` | `[nativeLibraryDir helpers (P4+), /system/bin]` — bundled shadows system |
| `LANG` | `C.UTF-8` |
| `TERM` | `dumb` |
| `ANDROID_ROOT`, `ANDROID_DATA` | passed through (`/system`, `/data` — Bionic/toybox expectations) |
| `GIT_CONFIG_NOSYSTEM` | `1` (P4+; never read system gitconfig) |

Caller-supplied `cmd.env` overlays after the scrub (explicit wins).

Once the git binary ships (P4), it appears in the Shell `PATH` for **local,
deny-net operations** (`git status`, `diff`, `commit`, `log`); network
subcommands fail with `EPERM` under the net-deny filter and the model is
prompted toward the structured `Git` tool.

The shell workspace root is a product boundary, not an OS sandbox boundary.
The prompt and receipt must be honest that `fs_confinement = app_uid_only`
(see D11 gating).

## Capability probing (eager, cached)

The superseded spec's "probe lazily" vs "cache must exist before sync
registration" contradiction is resolved as **eager** (D8):
`engine_mobile::build_mobile_engine` already owns the tokio runtime; it runs
`block_on(probe_android_capabilities())` before assembling the (synchronous)
tool registry. Results land in a `OnceCell`; `prepare()` and registration
gates only read the cache. Cache key: `packageVersionCode` + APK path.

Probed by real behavior (never inferred from API level):

1. minijail fork+exec smoke: `sh -c true` under `no_new_privs` + rlimits;
2. harmless seccomp filter installs in a disposable child (+ TSYNC);
3. **net-deny verification**: a forked probe child calls `socket()` directly
   expecting `EPERM` — no binary needed;
4. process-group kill works;
5. Landlock ABI (expected absent; recorded, not relied on);
6. system sh presence + `KSH_VERSION`;
7. **toybox applet inventory** (feeds the tool prompt);
8. bundled helper presence/version/hash (git at P4+, interpreter at P5).

## Shell tool (`tools/shell-mobile`)

- **Name: `Shell`** (D9). The prompt declares the mksh dialect (no process
  substitution, no `${var,,}`, no `mapfile`), embeds the probed applet
  inventory, and states the workspace root. No bashism bait via a `Bash` name.
- Schema: `{ command: string, timeout?: ms (desktop-capped), description?: string }`.
  No `run_in_background` in v1. Output truncation / max size mirror desktop
  BashTool rules.
- Pipeline:
  1. Permission check — desktop rules engine; tree-sitter-bash parses the
     command for allowlist/ask matching; **parse failure ⇒ ask**. The
     parser-dialect mismatch (bash grammar vs mksh execution) is a
     permission-UX concern only — the security boundary is minijail, never
     the parser. `AllowAlways` is session-only on mobile (verified; backlog:
     persistent mobile permission store).
  2. Network-intent advisory rejection: parsed heads such as `git clone`,
     `git fetch`, `git pull`, `git push`, `curl`, `wget`, `nc`, `ssh`, `scp`
     return a tool error pointing the model to the structured `Git` tool
     (when available) instead of burning a doomed deny-net execution.
     This list is **UX guidance, not a boundary** — evasion just hits the
     seccomp net-deny filter (`EPERM`). Parse failure cannot earn network.
     (bash's `/dev/tcp` does not exist in mksh; nothing to deny there.)
  3. Assemble the default deny-net mobile policy → `prepare()` → `run()` →
     receipt into metadata.

### Registration gates (engine-mobile)

Registered only when ALL hold (otherwise the tool is absent, not erroring):

1. Android platform present (`cfg(target_os = "android")`);
2. capability cache: minijail smoke + `no_new_privs` passed;
3. `MobileConfig.enable_shell` is on;
4. **secrets gate (D11)**: `AndroidShellConfig.secrets_in_keystore == true`
   (host attests credentials are Keystore-backed, no plaintext secrets under
   shell-readable app-private paths) **or**
   `AndroidShellConfig.shell_data_exposure_accepted == true` (explicit user
   acceptance);
5. `platform.process().is_available()`.

`register_mobile_tools` stays synchronous and reads the eager cache.

### Android inputs (Kotlin → Rust)

```rust
pub struct AndroidShellConfig {
    pub native_library_dir: PathBuf,     // bundled helper root (P4+)
    pub shell_workspace_root: PathBuf,
    pub app_cache_root: PathBuf,
    pub package_name: String,
    pub package_version_code: i64,       // probe cache key
    pub app_writable_roots: Vec<PathBuf>,// helper-path rejection set (P4+)
    pub enable_shell: bool,
    pub secrets_in_keystore: bool,       // D11 gate, host-attested
    pub shell_data_exposure_accepted: bool, // D11 gate, user-accepted
}
```

Bundled helper paths (P4+) must canonicalize under `native_library_dir` and
are rejected under any configured app-writable root. `MobileConfig` adds
`enable_shell` and embeds `AndroidShellConfig` (or an equivalent Android-only
block); the Android UniFFI constructors pass these fields explicitly so shell
registration is never enabled by target OS alone. The engine spawns children
itself (D6) — no helper-launch capability crosses the FFI.

## Git as the first structured network tool (P4; packaging proof P0b)

Networked git is deliberately not implemented as `Shell { command:
"git fetch ..." }`. It is a separate mobile-only `Git` tool with a structured
schema such as `{ operation, repo?, remote?, refspec?, path? }`. The tool maps
allowed operations to deterministic argv, builds an
`ExecTarget::BundledHelper` plan with `NetProfile::AllowNet`, and the
permission prompt says exactly which remote operation is being approved.
"Ask once" is session-only on mobile (verified) until a persistent mobile
permission store lands (backlog).

- **Packaging**: ship as `libgit.so` in `jniLibs` with
  `jniLibs.useLegacyPackaging = true` (`extractNativeLibs`) — an explicit,
  accepted APK-size cost, and the **first** W^X exec-packaging proof in the
  plan (P0b). Instrumentation tests assert exec from `nativeLibraryDir` AND a
  build-flag regression (so the flag cannot silently flip back).
- **Validation**: canonical path under `nativeLibraryDir`, rejected under
  app-writable roots; binary hash recorded in capabilities + receipts.
- **Build**: static NDK git; HTTPS via libcurl with an explicitly bundled CA
  store path (Android has no `/etc/ssl`). **Fallback, decided at P4 entry by
  "reproducible build + size ≤ budget" criteria**: a gitoxide-based native
  git tool (no exec path at all; MIT/Apache).
- **Licensing**: git is GPLv2 — OSS notices + source offer. Elsewhere prefer
  0BSD/MIT components (toybox, mksh, gitoxide).

## Failure semantics

All of the superseded spec's fail-closed cases carry over, with two changes:
errors must **name the unenforceable guarantee** (the new structured
`ProcessError` variants; never collapse policy failures into `Io`), and every
receipt lists `unsupported_required_features`. Anything requested but
unenforceable fails at `prepare()`; there is no runtime downgrade path.

## Rollout phases

| Phase | Content | Gate |
|---|---|---|
| **P0a** | NDK static libminijail+libcap linked into the engine `.so`; on-device smoke (fork child, `no_new_privs`, harmless seccomp, net-deny probe). **Build/link proof — no packaging** | **Global gate** — failure stops everything |
| **P1** | Trait extensions (D7, ProcessError, backend variant); `prepare()` + policy mapping; eager probe; `AndroidShellConfig` + UniFFI constructor migration; host tests. Execution stays disabled | Host tests green |
| **P2** | In-engine runner: system sh + system toybox; env scrub; pgid timeout/kill; receipts; first instrumentation matrix run | Device matrix green |
| **P3** | `Shell` tool: permission gating, deny-net policy, network-intent advisory, registration gates (incl. D11), prompt (dialect + applet inventory); behind `enable_shell` | Tool E2E on device |
| **P4** | Structured `Git` tool: P0b packaging proof (`libgit.so`, legacy extraction), NDK-git vs gitoxide decision, per-operation session-only network approval, GPLv2 compliance if shipping git; git enters Shell PATH for local deny-net ops | Packaging + tool E2E green |
| **P5** | Bundled mksh + toybox (`ExecTarget::BundledHelper`): version/dialect lock; system sh demoted to internal diagnostic fallback; prompt switches to fixed inventory | Device matrix green |

Backlog (explicitly out of v1): Landlock (when GKI ships it), full seccomp
syscall-allowlist policies (v1 uses only the fixed net-deny filter),
background tasks (phantom-process semantics first), brush (bash-grammar Rust
shell) interpreter upgrade, `isolatedProcess` exploration, persistent mobile
permission store, trampoline fallback shape (only if device soak disproves
in-engine fork discipline).

## Testing strategy

**Host unit**: policy mapping; every fail-closed case (LoopbackOnly,
writable/denied paths, `allow_subprocess=false`, `SystemShell` +
`NetworkPolicy::Allowed`); plan attachment; the runner's three rejections
(bypass tag / missing-or-wrong plan / wrong backend); receipt construction;
Shell network-intent advisory (`git clone`, `curl`, `git clone; nc`, parse
failure ⇒ ask, none earn network); structured Git argv mapping (P4+).

**Instrumentation matrix**: API 29 emulator (baseline), latest stable
emulator, ≥1 physical device, and a **non-debuggable build variant**
(debuggable SELinux posture is laxer and can mask exec restrictions).

**Device cases**: stdout/stderr capture and exit-code fidelity (non-zero ≠
transport failure); timeout kills the whole process group (command leaves an
orphan child on purpose); env scrub verified; net-deny via probe-child
`socket()` ⇒ `EPERM` (zero binary dependencies); capability probe snapshot;
gating (flag off ⇒ no tool; D11 gate unsatisfied ⇒ no tool; probe fail ⇒ no
tool); fork-discipline soak (concurrent shell calls under load, no deadlock);
phantom-process kill surfaces as a named error (best-effort case).

**Regression**: mirror the `platforms/posix` runner contract.
**P4 packaging**: per-ABI build, exec from `nativeLibraryDir`,
app-writable-root rejection, hash match, legacy-packaging flag regression,
**Git network approval does not grant Shell network access**.

## Risks

| Risk | Mitigation |
|---|---|
| **Phantom process killing** (Android 12+ caps app children at 32 system-wide; kills under pressure) | Kotlin-side foreground-service window during shell execution; concurrent-children cap; kills surface as named errors; documented |
| Parser/executor dialect mismatch (bash grammar parse, mksh execution) | Permission matching only — boundary is minijail; parse failure ⇒ ask; P5 dialect lock narrows it |
| mksh dialect vs model bash habits | `Shell` name + prompt dialect declaration + applet inventory; P5 locks dialect |
| fork inside the tokio runtime | `spawn_blocking` choke point; jail config compiled pre-fork; libminijail's async-signal-safe child path (its designed competence); device soak tests; trampoline documented as fallback shape if soak disproves |
| Plaintext secrets readable by shell (`PlainTextSecureStorage`, no FS confinement) | **D11 gate**: registration requires Keystore attestation or explicit user acceptance; Keystore migration tracked as its own work item |
| W^X / legacy packaging (git P4, interpreter P5) | Isolated to P0b/P4/P5; exec-from-`nativeLibraryDir` asserts; build-flag regression; app-writable-root rejection; gitoxide fallback |
| Network grant leakage to arbitrary commands | Shell has **no** network grant path at all; networked Git is structured argv only; tested explicitly |
| toybox applet drift across devices (no `awk` before Android 15, …) | Probed inventory feeds the prompt; CI matrix anchors API 29; disappears at P5 |
| No filesystem confinement in v1 | Receipts say `fs_confinement: app_uid_only`; D11 gate; Landlock backlogged |
| APK size growth (P4+) | Budgets: git ≤ ~8 MB/ABI, mksh+toybox ~1.5 MB/ABI; per-ABI CI tracking |
| GPLv2 compliance (git) | OSS notices + source offer; prefer 0BSD/MIT elsewhere |

## Resolved decisions from the superseded spec

1. *New tool vs adapted BashTool* → new `Shell` tool; BashTool's
   audited-bypass shape is structurally incompatible with the Android
   runner's invariants (see Context).
2. *Landlock absence disables shell entirely vs only strict FS policies* →
   only strict FS policies fail. Shell ships without filesystem confinement
   only behind the D11 secrets gate; receipts say
   `fs_confinement: app_uid_only`.
3. *Tier 2 `/system/bin/sh` exposure* → the tier model is replaced:
   interpreter provenance (system → bundled) is a **phase axis**, bundled
   helpers are an orthogonal axis. System sh is the v1 interpreter and becomes
   an internal-diagnostics fallback after P5. No user-facing "diagnostic
   shell" setting exists.

## References

- Superseded: `docs/superpowers/specs/2026-06-12-android-minijail-sandbox-design.md`
- Minijail analysis: `third_party/minijail/graphify-out/GRAPH_REPORT.md`
- [Android 10 behavior changes: removed execute permission for app home directory](https://developer.android.com/about/versions/10/behavior-changes-10#execute-permission)
- Codebase anchors: `traits/src/sandbox.rs`, `traits/src/process.rs`,
  `tools/shell/src/bash.rs`, `apps/engine-mobile/src/lib.rs`,
  `apps/engine-mobile/src/host.rs:287,342-344`,
  `platforms/android/src/lib.rs`
