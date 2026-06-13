# Android Bundled Shell Design (P5)

Date: 2026-06-13
Status: Approved design (brainstormed + section-approved)
Part of: the Android sandbox + shell effort (spec r3
`2026-06-12-android-sandbox-shell-design.md`, §P5 of the rollout). This is the
P5 phase — bundling the shell interpreter + utilities — in its own spec because
it is the **first phase that ships executables**, which raises the Android 10
W^X exec-packaging problem that P2–P4 all sidestepped.

## Summary

P5 replaces the v1 reliance on the **device's** `/system/bin/sh` (mksh) + system
toybox with **bundled, version-locked mksh + toybox** shipped in the APK. Both
are compiled with the NDK and packaged under `nativeLibraryDir` (named
`lib*.so`, `useLegacyPackaging=true`) so they are executable under Android 10+
W^X — the **first real exec-packaging proof** in the whole effort (the deferred
"P0b"). The mobile `Shell` tool's `prepare()` switches its exec target from
`SystemShell` to `ExecTarget::BundledHelper{mksh}` (the seam P2 already built),
with `PATH` pointed at the bundled toybox applets; everything else (minijail
jail, deny-net seccomp, timeout, identity hash) is unchanged.

This delivers P5's only real payoff over the working P3 system-sh path:
**consistency** — a locked interpreter version + dialect and a fixed applet
inventory, eliminating device fragmentation (API 29 has no `awk`; toybox/mksh
behavior drifts by OS/OEM). The dialect stays **mksh** (continuous with system
sh), so the P3 tool prompt is essentially unchanged.

## Decision log (this P5 brainstorm, 2026-06-13)

| # | Decision | Choice |
|---|---|---|
| B1 | What to bundle | **mksh + toybox** (two binaries) — NOT busybox (its `ash` would change the dialect from the mksh P3 declares), NOT toybox-only (would leave the interpreter unlocked) |
| B2 | System sh after bundling | **Replace (bundled-only)**: when bundling is enabled+ready the Shell uses bundled mksh+toybox exclusively; if the bundle probe fails the Shell tool is **not registered** (absent-not-erroring). NO runtime fallback to system sh — that would reintroduce the fragmentation P5 removes + a second untested path. (System sh remains only the separate "bundled Shell not enabled" P3 configuration, not a fallback.) |
| B3 | toybox applet set | **Full** (locked `.config`, multi-call ~1 MB) — no curated subset (curation adds complexity for little size win) |
| B4 | applet resolution under W^X | **Symlink farm** (app-startup symlinks `grep`/`sed`/… → `nativeLibraryDir/libtoybox.so`, PATH points there; toybox dispatches on argv[0]). **P5a (2026-06-13) PROVED this works on a real device** (Android 14, arm64). The command-rewrite alternative (`libtoybox.so <applet>`) was **disproven** — toybox dispatches on argv[0]'s basename, so a leading applet arg yields `toybox: Unknown command`; rewrite is NOT a viable fallback (would need argv[0] spoofing). Symlink farm is the sole mechanism. |
| B5 | Packaging proof | **Front-loaded as P5a** (the global gate, like P0a for minijail) — the W^X exec-packaging risk is the most likely failure |
| B6 | Vendoring | mksh (AOSP source) + toybox (upstream release) vendored into `third_party/` as real committed dirs (G4 convention, like libcap/minijail/git2-rs) |

## Context

- Predecessor: P0a–P4 merged to main (`13788128`). The minijail runner execs a
  **system** `/system/bin/sh -c` jailed + deny-net (P2/P3, device-verified); the
  mobile `Shell` tool is registration-gated (P3). P2 already built
  `ExecTarget::BundledHelper{name,path,hash}` + canonical-path-under-
  nativeLibraryDir + content-hash identity checking + the jailed run path — P5
  is the **first consumer** of that bundled-helper seam.
- Why P5 is worth doing despite P3 working on system sh: **consistency**. System
  toybox lacks `awk` before Android 15 and varies by OEM; system mksh's dialect
  drifts. A locked bundle makes the tool's behavior (and its prompt's applet
  inventory) identical across devices and testable in CI against the exact bits
  that ship.
- Spec r3 D5 named this phase ("bundled mksh/toybox version-lock; system sh
  demoted to internal diagnostic fallback"). This brainstorm refines D5's
  "fallback" to **replace** (B2): no runtime fallback.
- Licenses: toybox = **0BSD** (no obligations); mksh = **MirOS/ISC-style
  permissive**. OSS notices only; no GPL burden (unlike GNU coreutils/bash).

## Goals

1. Ship version-locked mksh + toybox executables, packaged under
   `nativeLibraryDir` and proven executable under Android 10+ W^X (the deferred
   P0b packaging proof).
2. Switch the `Shell` tool to the bundled interpreter via the existing
   `ExecTarget::BundledHelper` seam, with no change to the jail/deny-net/timeout
   behavior and (dialect being mksh) minimal prompt change.
3. Eliminate device fragmentation: a fixed applet inventory + locked dialect, so
   the prompt is exact and the behavior is CI-reproducible.
4. Stay fail-closed / absent-not-erroring: bundle probe fails → Shell tool not
   registered; no silent downgrade to system sh.

## Non-goals

1. No busybox / `ash` (B1 keeps the mksh dialect). No curated applet subset (B3).
2. No runtime fallback to system sh (B2). The system-sh path persists only as
   the pre-existing "bundled Shell not enabled" P3 configuration.
3. No change to the minijail jail, deny-net seccomp, timeout/pgid-kill, or
   identity-hash checking (all inherited unchanged from P2/P3).
4. No stdin wiring (still the P2-deferred NULL-stdin marker). No background
   tasks (non-goal since P2).
5. No SSH/network — the Shell stays deny-net; networked git is the separate P4
   Git tool.

## Architecture

```text
third_party/
├── libcap/  minijail/  git2-rs/  libgit2/   (P0a-P4, vendored)
├── mksh/        ◀ P5: vendored AOSP mksh source (NDK cross-compiled)
└── toybox/      ◀ P5: vendored toybox upstream release + locked .config

lingxi-code/
├── platforms/android-shellbin/   CREATE: build crate (cc/make → 2 executables)
│   └── build.rs   cfg(target_os="android"): NDK-compile mksh + toybox; host no-op
├── tools/shell-mobile/src/lib.rs MODIFY: prepare() → BundledHelper{mksh} when bundled-ready
├── platforms/android/src/capabilities.rs MODIFY: bundled_shell_exec probe + fixed applet inventory + mksh version
├── tool-api/src/builtin_context.rs MODIFY: AndroidShellToolCtx gains bundled fields (paths/hash/applets)
└── apps/android-aar/src/lib.rs   MODIFY: bundled paths + the added gate conjunct + applet-symlink bootstrap

packaging: the two executables ship as jniLibs/<abi>/libmksh.so + libtoybox.so
           with useLegacyPackaging=true → PM extracts them executable into
           nativeLibraryDir. toybox is a multi-call binary (one file = all applets).
```

- **Executables, not libraries (the W^X crux).** Unlike libgit2 (P4, a library)
  or libminijail (P0a, a library), mksh and toybox are **executables**. Android
  10+ forbids `execve` of files in app-writable dirs; the only legal path is
  `nativeLibraryDir`, populated by naming the files `lib*.so` and setting
  `useLegacyPackaging=true` (`extractNativeLibs`) so the package manager extracts
  them as real, executable on-disk files. **P5a proves this on-device** — it is
  the deferred P0b and the most likely failure point.
- **Execution reuses the P2 seam (zero new architecture).** The Shell tool's
  `prepare()` builds `ExecTarget::BundledHelper{ name:"mksh",
  path:<nativeLibraryDir>/libmksh.so, hash }`, `argv = ["sh","-c", command]`,
  `env.PATH = [<applet-dir>, /system/bin]`. The runner's jailed-spawn,
  net-deny seccomp, timeout/pgid-kill, and identity-hash check are all the
  existing P2 `BundledHelper` path — unchanged.
- **applet resolution under W^X (B4).** mksh running `grep foo` must find a
  `grep` program. toybox is one multi-call binary at
  `nativeLibraryDir/libtoybox.so`; that dir is read-only and app-writable dirs
  are noexec. **Primary: a symlink farm** — at app startup the Kotlin/FFI
  bootstrap creates `grep`/`sed`/`find`/… symlinks (one per applet) in an
  app-private dir → `nativeLibraryDir/libtoybox.so`, and `PATH` points there.
  `execve` resolves the symlink to the libtoybox.so inode (in the executable
  `nativeLibraryDir`) and runs it with `argv[0]` = the applet name, on which
  toybox dispatches. W^X/SELinux check the **target** inode (executable), not the
  symlink, so it should pass — **P5a verifies this on-device**. **Fallback
  (documented): command rewrite** — the Shell layer rewrites a bare applet head
  to `libtoybox.so <applet>`; used only if the symlink approach fails the device
  probe. (Rewrite is the less-preferred path because rewriting arbitrary shell
  reintroduces the parsing fragility P3 deliberately avoided in net-intent.)

## The Shell tool change (minimal)

`ShellMobileTool::call` (P3) currently prepares `ExecTarget::SystemShell`. P5:

- When the ctx indicates the bundle is ready (see Gating), `prepare()` targets
  `ExecTarget::BundledHelper{ name:"mksh", path, hash }` with `argv =
  ["sh","-c", command]` and `env.PATH = [<applet-symlink-dir>, /system/bin]`
  (bundled applets shadow system).
- Unchanged: the deny-net `SandboxPolicy`, the timeout, the minijail jail, and
  the `BundledHelper` identity-hash check.
- Prompt: dialect remains **mksh** (B1 → the P3 prompt text is essentially
  unchanged); the applet inventory switches from "probed system toybox" to the
  **fixed bundled inventory** (compile-time known, identical across devices).

## Capability probe changes

- New probe item `bundled_shell_exec`: `libmksh.so` `execve`s from
  `nativeLibraryDir` (jailed `mksh -c true` exits 0) AND applet resolution works
  (`grep` via the symlink farm — or the rewrite fallback — exits 0). This is the
  runtime form of the P0b proof.
- The applet inventory becomes the **fixed locked list** of the bundled toybox
  `.config` (compile-time known) instead of parsing system `toybox --help`.
- The reported shell version becomes the bundled mksh version string (replacing
  P3's system `KSH_VERSION` probe).

## Registration gate (B2 — replace / bundled-only)

The existing P3 Shell gate — `enable_shell && D11 secrets gate && minijail caps
(smoke + no_new_privs)` — gains one AND:

```
bundled_shell_ready =
    libmksh.so + libtoybox.so present under nativeLibraryDir
    && bundled_shell_exec probe passed (P0b runtime: execve + applet resolution)
```

All pass → Shell registers and `prepare()` targets bundled mksh. Any fail → the
Shell tool is **not registered** (absent-not-erroring; **no fallback to system
sh**). `AndroidShellToolCtx` gains bundled fields (mksh path + hash, applet-dir,
fixed applet inventory); the gate is computed in `android-aar`
`build_android_engine`, mirroring the P3 `android_shell_gate` pattern. D11 and
the minijail caps remain required (the Shell still reads app-private files and
still runs jailed).

## Rollout phases

- **P5a — cross-compile + W^X packaging proof (global gate).** Vendor mksh
  (AOSP) + toybox (upstream) into `third_party/`; `platforms/android-shellbin`
  build crate NDK-compiles both (arm64 + x86_64), toybox with a locked `.config`
  (full applet set). Package as `libmksh.so`/`libtoybox.so` +
  `useLegacyPackaging`. On-device prove: (1) `libmksh.so` execve from
  `nativeLibraryDir` succeeds; (2) applet resolution works (symlink farm →
  else command-rewrite fallback). Any failure → stop here; the Shell stays on
  the P3 system-sh configuration.
- **P5b — Shell switch + probe + gate (host-testable parts).**
  `ShellMobileTool::prepare` → `BundledHelper{mksh}` + applet PATH;
  `capabilities` gets `bundled_shell_exec` + the fixed applet inventory + mksh
  version; `AndroidShellToolCtx` gains the bundled fields; android-aar computes
  the added gate conjunct + the applet-symlink bootstrap. Host tests: plan
  construction (BundledHelper target + PATH), gating (ready→register /
  missing→absent), fixed inventory in the prompt, deny-net policy unchanged.
- **P5c — device acceptance + P5 gate.** On-device: bundled `mksh -c` runs
  toybox commands (`echo | grep`, `sed`, `find`) end-to-end; deny-net still
  blocks `socket()`; timeout kills the process group; the bundled path never
  touches system sh. Workspace fmt/clippy/test + both-ABI cross-build + AAR size
  record.

## Testing strategy

- **Host unit**: `prepare()` produces `BundledHelper{mksh,path,hash}` + PATH with
  the applet dir; gating (`bundled_ready` true→register / false→absent); fixed
  applet inventory injected into the prompt; deny-net `SandboxPolicy` unchanged;
  path/hash validation reuses the P2 `BundledHelper` tests.
- **Build**: mksh + toybox per-ABI NDK cross-compile; packaged as `lib*.so`;
  symbol/format check (ELF executable).
- **Device acceptance (API-34 arm64 emulator)**: `libmksh.so` execve; `echo x |
  grep x` (applet resolution), `sed`, `find`; deny-net `socket()` → EPERM (the
  P2 net-deny BPF still applies); timeout kills pgid; the bundled path does not
  reach system sh.
- **Fallback branch**: if symlink-farm execve fails on-device, the command-
  rewrite layer takes over and the same applet commands are re-verified.

## Risks

| Risk | Mitigation |
|---|---|
| **W^X / nativeLibraryDir execve (first real P0b proof)** | P5a front-loaded global gate; the libgit2/libcap `.so` packaging precedent; failure → Shell stays on P3 system-sh, P5 does not ship |
| Symlink execve blocked by SELinux/W^X (applet resolution) | P5a decides on-device; command-rewrite documented fallback |
| Phantom-process kill of bundled children (Android 12+) | Same as P2/P3: foreground-service window + named error (unchanged) |
| APK size (two binaries) | budget ~1.5 MB/ABI (mksh small + toybox ~1 MB); per-ABI CI tracking |
| toybox/mksh NDK cross-compile friction | toybox officially supports the Android NDK (it ships in AOSP); mksh has an AOSP `Android.bp`; P5a front-loads the proof |
| Device execve unverifiable on host | host tests cover plan/gate/probe mapping; execve + applet resolution are the P5c device acceptance (same posture as P2) |

## Open decisions

1. The applet-resolution mechanism (symlink farm vs command rewrite) is resolved
   at P5a by the on-device probe, not in advance.
2. Whether a future phase ever re-enables a system-sh diagnostic fallback (B2
   chose replace) — revisit only if field data shows bundled-exec failures common
   enough to warrant a degraded mode.

## References

- Parent: `docs/superpowers/specs/2026-06-12-android-sandbox-shell-design.md`
  (r3, §Rollout P5 + D5).
- P2 `ExecTarget::BundledHelper` seam (the bundled-helper exec path P5 consumes):
  `traits`/`platforms/android` (merged in the P2 work).
- P3 Shell tool + `AndroidShellToolCtx` + `android_shell_gate` (the patterns P5
  extends): `tools/shell-mobile/`, `apps/android-aar/src/lib.rs`.
- Vendoring precedents: `third_party/libcap`, `third_party/minijail`,
  `third_party/git2-rs` (all real committed dirs, G4 convention).
