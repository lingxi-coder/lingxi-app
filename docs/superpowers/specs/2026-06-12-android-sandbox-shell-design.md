# Android Sandbox & Shell Design

Date: 2026-06-12
Status: Approved design (brainstormed + section-approved)
Supersedes: `2026-06-12-android-minijail-sandbox-design.md`

## Summary

Android gets a real shell tool through a **phased, fail-closed hardened process
runtime**: v1 executes the device's own `/system/bin/sh` (mksh) and system
toybox utilities under Minijail restrictions applied **in-engine** via
`minijail_run()` — no bundled trampoline executable, no FD framing protocol.
`git` is the first *bundled* helper and the only component that needs the
Android 10 W^X exec-packaging path in v1. A later phase swaps in a bundled
mksh + toybox to lock the interpreter version and dialect across devices.

The model-facing tool is a new mobile-only `Shell` tool (mksh dialect declared
in its prompt, probed toybox applet inventory embedded). Desktop `BashTool` is
untouched. Permission gating reuses the desktop allowlist/ask rules engine;
network access is granted per helper class (text tools deny-net via seccomp,
git network subcommands ask-once then allow).

## Decision log

Decisions locked during brainstorming (2026-06-12):

| # | Decision | Choice |
|---|---|---|
| D1 | Output scope | Revise + complete the minijail spec into one sandbox + shell spec |
| D2 | Product target | Repo coding workflow: toybox base set + git, phased |
| D3 | Tool input form | Real shell interpreter, command-string input |
| D4 | Permission model | Reuse desktop allowlist/ask rules; per-helper network tiers |
| D5 | Interpreter strategy | Phased: v1 system sh + system toybox → git bundled → bundled mksh/toybox version-lock; system sh demoted to internal diagnostic fallback afterward |
| D6 | Exec architecture | In-engine `minijail_run()`; no trampoline executable, no FD protocol |
| D7 | Plan carriage | New private `plan: Option<BackendPlanHandle>` field on `SandboxedCommand`, NOT a `SandboxedTag` variant change |
| D8 | Probe timing | Eager: `block_on(probe())` inside `build_mobile_engine` before synchronous tool registration |
| D9 | Tool name | `Shell` (not `Bash`) — dialect honesty over habit transfer |
| D10 | Network granularity | One policy per `sh -c` process tree; any net-requiring segment ⇒ whole tree allowed, recorded in receipt |

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

Android platform facts the design rests on:

- **W^X applies to `execve`, not to library loading.** An APK-shipped `.so`
  can be linked/loaded into the engine regardless of `extractNativeLibs`;
  only on-disk *executables* need the package-manager-extracted
  `nativeLibraryDir` path. This is what removes the trampoline (D6) and
  confines exec-packaging risk to git (P4).
- `/system/bin/sh` is **mksh**; system utilities are **toybox** (Android 6+).
  Applet inventory drifts by OS version/OEM (e.g. no `awk` before Android 15).
- **Landlock is effectively absent** from shipping Android kernels (GKI does
  not enable it). v1 therefore offers no filesystem confinement; the app's
  own private-directory sandbox is the filesystem boundary.
- **seccomp cannot dereference path arguments**, so `execve` cannot be
  filtered by target path: any real interpreter can exec `/system/bin/*`.
  The "known binaries" threat model of the superseded spec is abandoned
  deliberately (D3); constraints are syscall/resource-level.
- **Phantom process killing** (Android 12+): the OS caps app child processes
  (32 system-wide default) and kills them under memory pressure or excessive
  CPU while cached. Affects every child this design spawns.

## Threat model

This runtime constrains **what the executed command can do** — resource
ceilings, `no_new_privs`, network denial, environment hygiene, lifetime
control — protecting against a misbehaving or model-misdriven tool process.
It does **not** protect against compromised app code: every child shares the
app UID and the app's own authority. True privilege separation
(`isolatedProcess`) was considered and rejected for v1: it cannot exec
PM-installed binaries, has no usable filesystem access, and forces a
Binder-brokered I/O model; it remains a backlog exploration.

## Goals

1. A real `Sandbox`/`ProcessRunner` implementation for Android behind the
   existing trait seam, with `Sandbox::prepare()` as the only admission path.
2. v1 shell capability with zero exec-packaging risk (system sh + toybox).
3. Fail closed: any requested-but-unenforceable policy is rejected at
   `prepare()` with a named guarantee; no silent downgrade, ever.
4. Honest receipts: every execution records what was actually enforced.
5. Capability-gated tool registration (not `target_os` checks).

## Non-goals

1. No desktop-parity sandbox claims on Android (no namespaces, no chroot,
   no mount control, no network namespace).
2. No filesystem confinement in v1 (no Landlock on shipping kernels; path
   validation is input hygiene, not a boundary).
3. No binary-identity ("known binaries") guarantees — abandoned with D3.
4. No arbitrary user-downloaded binary execution.
5. No background shell tasks in v1 (needs phantom-process semantics first).
6. No iOS (separate design).

## Architecture

```text
┌─ Engine process (Rust .so inside the Android app) ──────────────┐
│  ShellTool (tools/shell-mobile)                                 │
│    │ permission rules (desktop engine) + helper net profile     │
│    ▼                                                            │
│  AndroidMinijailSandbox::prepare(cmd, &policy)   [sync]         │
│    ▼  SandboxedCommand{ Wrapped{AndroidMinijail}, plan }        │
│  AndroidMinijailProcessRunner::run                              │
│    │  spawn_blocking → minijail_run()                           │
│    │    ├ fork ───────────────────────────────┐                 │
└────┼──────────────────────────────────────────┼─────────────────┘
     │                                          ▼ (child)
     │                       no_new_privs + rlimits (+ net-deny
     │                       seccomp when policy demands) → setsid
     │                       → execve(/system/bin/sh -c …   [v1]
     │                                | nativeLibraryDir/git [P4])
     └── stdio pipes via run_remap back to the engine
```

libminijail (+ a static libcap) is built with the NDK and linked into the
engine `.so` via `minijail-sys` with pre-generated bindings. Library loading
is exempt from W^X, so **P0a (the global gate) is a build/link/smoke proof,
not a packaging proof**. The Android impls are `cfg(target_os = "android")`
gated; host builds keep the existing stubs so the crate still compiles and
tests on macOS/Linux.

### Module layout

```text
lingxi-code/platforms/android/src/
├── sandbox.rs        AndroidMinijailSandbox (traits::Sandbox)
├── process.rs        AndroidMinijailProcessRunner (traits::ProcessRunner)
├── capabilities.rs   one-shot probe + session cache
├── policy.rs         SandboxPolicy → AndroidSandboxPlan mapping + helper profiles
└── receipt.rs        AndroidSandboxReceipt

lingxi-code/tools/shell-mobile/   ShellTool (new crate; desktop BashTool untouched)
```

The superseded spec's `helper_protocol.rs` is dropped with the trampoline.

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
no benefit — the plan rides next to the tag, not inside it.

### AndroidSandboxPlan (in-process only, never serialized)

```rust
pub enum ExecTarget {
    SystemShell,                                  // /system/bin/sh -c <command>
    BundledHelper { name: String, path: PathBuf, hash: String },  // P4+
}

pub struct AndroidSandboxPlan {
    pub target: ExecTarget,
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,    // post-scrub allowlist result
    pub network: NetProfile,           // DenyNet | AllowNet
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
| `network = Allowed` | no network restriction |
| `network = LoopbackOnly` | `Unavailable("loopback-only network policy is unenforceable on Android")` |
| `writable_paths` / `denied_paths` non-empty | `Unavailable("filesystem confinement requires Landlock; not available")` — the default mobile policy requests none |
| `allow_subprocess = false` | `PolicyUnsupported` — both sh and git require children |
| `limits.max_cpu_seconds` | `RLIMIT_CPU` (wall-clock timeout stays authoritative in the runner) |
| `limits.max_memory_mb` | `RLIMIT_AS`, best effort (device-dependent; receipt says so) |
| `limits.max_open_files` | `RLIMIT_NOFILE` |
| `limits.max_processes` | not mapped in v1 — `RLIMIT_NPROC` is UID-scoped and would count the whole app |
| (always) | `RLIMIT_CORE = 0`, `no_new_privs` |

The **default mobile policy** (assembled by the shell tool) is: no
filesystem confinement, network per helper profile, subprocesses allowed,
default limits. Filesystem reality: the app-private directory (the OS app
sandbox) is the boundary; the receipt records `fs_confinement: none`.

## AndroidMinijailProcessRunner

Security invariants, checked before anything runs:

1. **Reject `SandboxedTag::BypassAuditedWithReason`** — the desktop
   `BashTool` shape dies here by construction.
2. Reject `Wrapped` whose backend ≠ `AndroidMinijail`.
3. Reject missing plan / failed downcast ⇒ `MalformedSandboxPlan`.

Execution:

- `minijail_run()` from a `spawn_blocking` closure (fork never happens on an
  async worker thread). Minijail's post-fork child path is async-signal-safe
  by design; residual fork-in-threaded-runtime risk is covered by device soak
  tests.
- Child setup: `no_new_privs` → rlimits → optional net-deny seccomp →
  `setsid()` (own session/process group) → `execve`.
- stdio: `run_remap` pipes (Minijail's default maps stdio to `/dev/null` —
  the superseded spec's risk #6 stands).
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
| `HOME` | app-private files root (the workspace) |
| `TMPDIR` | app cache dir |
| `PATH` | `[bundled helper dir (when present), /system/bin]` — bundled shadows system |
| `LANG` | `C.UTF-8` |
| `TERM` | `dumb` |
| `ANDROID_ROOT`, `ANDROID_DATA` | passed through (`/system`, `/data` — Bionic/toybox expectations) |
| `GIT_CONFIG_NOSYSTEM` | `1` (P4+; never read system gitconfig) |

Caller-supplied `cmd.env` overlays after the scrub (explicit wins).

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
3. **net-deny verification**: the forked probe child calls `socket()`
   directly expecting `EPERM` — no binary needed;
4. process-group kill works;
5. Landlock ABI (expected absent; recorded, not relied on);
6. system sh presence + `KSH_VERSION`;
7. **toybox applet inventory** (feeds the tool prompt);
8. bundled git presence/version/hash (P4+).

## Shell tool (`tools/shell-mobile`)

- **Name: `Shell`** (D9). The prompt declares the mksh dialect (no process
  substitution, no `${var,,}`, no `mapfile`), embeds the probed applet
  inventory, and states the workspace root. No bashism bait via a `Bash` name.
- Schema: `{ command: string, timeout?: ms (desktop-capped), description?: string }`.
  No `run_in_background` in v1. Output truncation / max size mirror desktop
  BashTool rules.
- Pipeline: ① permission check — desktop rules engine; tree-sitter-bash
  parses the command for allowlist/ask matching; **parse failure ⇒ ask**.
  The parser-dialect mismatch (bash grammar vs mksh execution) is a
  permission-UX concern only — the security boundary is minijail, never the
  parser. ② network classification (D10): if any parsed pipeline segment
  head is `git` with a network subcommand (`clone`, `fetch`, `pull`, `push`,
  `ls-remote`, `remote update`, `submodule update`) ⇒ ask-once ("git network
  access", persisted via the existing allowlist store) ⇒
  `NetworkPolicy::Allowed`; otherwise `Disabled`. One `sh -c` = one process
  tree = one network policy; receipts record the grant. ③ assemble the
  default mobile policy → `prepare()` → `run()` → receipt into metadata.

### Registration gates (engine-mobile)

Registered only when ALL hold (otherwise the tool is absent, not erroring):

1. Android platform present (`cfg(target_os = "android")`);
2. capability cache: minijail smoke + `no_new_privs` passed;
3. `MobileConfig.enable_shell` is on;
4. `platform.process().is_available()`.

`register_mobile_tools` stays synchronous and reads the eager cache.

### Android inputs (Kotlin → Rust)

```rust
pub struct AndroidShellConfig {
    pub native_library_dir: PathBuf,     // bundled helper root (P4+)
    pub package_name: String,
    pub package_version_code: i64,       // probe cache key
    pub app_writable_roots: Vec<PathBuf>,// helper-path rejection set (P4+)
    pub enable_shell: bool,
}
```

Helper paths (P4+) must canonicalize under `native_library_dir` and are
rejected under any configured app-writable root.

## git as the first bundled helper (P4; packaging proof P0b)

- **Packaging**: ship as `libgit.so` in `jniLibs` with
  `jniLibs.useLegacyPackaging = true` (`extractNativeLibs`) — an explicit,
  accepted APK-size cost. This is the only W^X-exposed component in the
  design. Instrumentation tests assert exec from `nativeLibraryDir` AND a
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
| **P0a** | NDK static libminijail+libcap into the engine `.so`; on-device smoke (fork child, `no_new_privs` + harmless seccomp) | **Global gate** — failure stops everything |
| **P1** | Trait extensions (D7, ProcessError, backend variant); `prepare()` + policy mapping; eager probe; host tests. Execution stays disabled | Host tests green |
| **P2** | Runner via `minijail_run`: system sh + system toybox; env scrub; pgid timeout/kill; receipts; first instrumentation matrix run | Device matrix green |
| **P3** | `Shell` tool: permission/net gating, registration gates, prompt (dialect + applet inventory); behind `enable_shell` | Tool E2E on device |
| **P4** | git bundled helper: P0b packaging proof, exec test, ask-once network, GPLv2 compliance; NDK-git vs gitoxide decision | Packaging tests green |
| **P5** | Bundled mksh + toybox: version/dialect lock; system sh demoted to internal diagnostic fallback; prompt switches to fixed inventory | Device matrix green |

Backlog (explicitly out of v1): Landlock (when GKI ships it), full seccomp
syscall-allowlist policies (v1 uses only the fixed net-deny filter),
background tasks (phantom-process semantics first), brush (bash-grammar Rust
shell) interpreter upgrade, `isolatedProcess` exploration.

## Testing strategy

**Host unit**: policy mapping; every fail-closed case (LoopbackOnly,
writable/denied paths, `allow_subprocess=false`); plan attachment; the
runner's three rejections (bypass tag / missing-or-wrong plan / wrong
backend); receipt construction; network classifier (`git clone` vs
`git status` vs `grep | sed` pipelines).

**Instrumentation matrix**: API 29 emulator (baseline), latest stable
emulator, ≥1 physical device, and a **non-debuggable build variant**
(debuggable SELinux posture is laxer and can mask exec restrictions).

**Device cases**: stdout/stderr capture and exit-code fidelity (non-zero ≠
transport failure); timeout kills the whole process group (command leaves an
orphan child on purpose); env scrub verified; net-deny via probe-child
`socket()` ⇒ `EPERM` (zero binary dependencies); capability probe snapshot;
gating (flag off ⇒ no tool; probe fail ⇒ no tool); phantom-process kill
surfaces as a named error (best-effort case).

**Regression**: mirror the `platforms/posix` runner contract.
**P4 packaging**: per-ABI build, exec from `nativeLibraryDir`,
app-writable-root rejection, hash match, legacy-packaging flag regression.

## Risks

| Risk | Mitigation |
|---|---|
| **Phantom process killing** (Android 12+ caps app children at 32 system-wide; kills under pressure) | Kotlin-side foreground-service window during shell execution; concurrent-children cap; kills surface as named errors; documented |
| Parser/executor dialect mismatch (bash grammar parse, mksh execution) | Permission matching only — boundary is minijail; parse failure ⇒ ask; P5 dialect lock narrows it |
| mksh dialect vs model bash habits | `Shell` name + prompt dialect declaration + applet inventory; P5 locks dialect |
| fork inside the tokio runtime | `spawn_blocking` choke point; minijail's async-signal-safe child path; device soak tests |
| W^X / legacy packaging (git only) | Isolated to P0b/P4; build-flag regression assert; gitoxide fallback |
| Network policy coarseness (one grant per process tree) | Honest receipts; per-segment split only if engine-built pipelines ever land |
| toybox applet drift across devices (no `awk` before Android 15, …) | Probed inventory feeds the prompt; CI matrix anchors API 29; disappears at P5 |
| No filesystem confinement in v1 | Real boundary = app-private dir (OS app sandbox); receipts say `fs_confinement: none`; Landlock backlogged |
| APK size growth (P4+) | Budgets: git ≤ ~8 MB/ABI, mksh+toybox ~1.5 MB/ABI; per-ABI CI tracking |
| GPLv2 compliance (git) | OSS notices + source offer; prefer 0BSD/MIT elsewhere |

## Resolved decisions from the superseded spec

1. *New tool vs adapted BashTool* → new `Shell` tool; BashTool's
   audited-bypass shape is structurally incompatible with the Android
   runner's invariants (see Context).
2. *Landlock absence disables shell entirely vs only strict FS policies* →
   only strict FS policies fail; shell ships without filesystem confinement
   (anything else means shell never ships — Landlock is absent on real
   devices).
3. *Tier 2 `/system/bin/sh` exposure* → the tier model is replaced: interpreter
   provenance (system → bundled) is a **phase axis**, bundled helpers are an
   orthogonal axis. System sh is the v1 interpreter and becomes an
   internal-diagnostics fallback after P5. No user-facing "diagnostic shell"
   setting exists.

## References

- Superseded: `docs/superpowers/specs/2026-06-12-android-minijail-sandbox-design.md`
- Minijail analysis: `third_party/minijail/graphify-out/GRAPH_REPORT.md`
- [Android 10 behavior changes: removed execute permission for app home directory](https://developer.android.com/about/versions/10/behavior-changes-10#execute-permission)
- Codebase anchors: `traits/src/sandbox.rs`, `traits/src/process.rs`,
  `tools/shell/src/bash.rs`, `apps/engine-mobile/src/lib.rs`,
  `platforms/android/src/lib.rs`
