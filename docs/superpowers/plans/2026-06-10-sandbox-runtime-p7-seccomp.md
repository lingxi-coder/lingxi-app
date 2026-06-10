# sandbox-runtime P7 — seccomp AF_UNIX block (apply-seccomp via seccompiler)

> REQUIRED SUB-SKILL: superpowers:subagent-driven-development.

**Goal:** Fill the P4-2c seccomp seam. The package ships a pre-built C `apply-seccomp` binary (source not vendored) whose documented contract is: block `socket(AF_UNIX, …)` inside the sandbox so the workload cannot create its own unix sockets to reach the bridge sockets directly, forcing all egress through the TCP proxy listeners (defense-in-depth on top of `--unshare-net`). The faithful Rust equivalent (per the umbrella spec) is a small `apply-seccomp` binary built with `seccompiler` (Firecracker's pure-Rust BPF compiler) + `nix`: set `NO_NEW_PRIVS`, install a seccomp BPF that returns `EPERM` for `socket(AF_UNIX)` (and `socketpair(AF_UNIX)`), then `execvp` the workload. Wire `resolve_apply_seccomp_prefix` to locate/use it. Docker-verify: in-sandbox `socket(AF_UNIX)` → EPERM while `socket(AF_INET)` still works.

**Reference:** `docs/superpowers/references/sandbox-runtime-0.0.54/dist/sandbox/generate-seccomp-filter.js` (the binary locator: `getApplySeccompBinaryPath`, arch x64/arm64 only, the explicit-path → local-vendor → global-npm search order) + the behavioral contract documented in `linux-sandbox-utils.js` (apply-seccomp blocks `socket(AF_UNIX)`; x86/ia32 unsupported because `socketcall()` isn't blocked; nested userns/PID1 is for /proc + reaping).

**Scope note (documented divergence):** the original binary ALSO sets up a nested user+PID+mount namespace, remounts /proc, and becomes a PID-1 reaper. In our `wrap_command_with_sandbox_linux` assembly, bwrap already provides `--unshare-pid --proc` (the parent PID ns + fresh /proc), so the SECURITY-ESSENTIAL part — the `socket(AF_UNIX)` seccomp block — is what `apply-seccomp` must reproduce faithfully; the nested-ns/reaper aspects are provided by the surrounding bwrap args. Document this clearly. (The C source is not available to copy; we port the documented behavioral contract.)

**Deps:** add `seccompiler` (pure Rust, no C) to the new bin crate; `nix` 0.27 (in lock) for `prctl`/`execvp`. The bin crate is Linux-only (`#[cfg(target_os="linux")]` / a `[target.'cfg(target_os=\"linux\")'.dependencies]` gate); on non-Linux the binary is a build-excluded or stub.

**Branch:** `parity-sandbox-runtime-p7`. **Conventions:** Cargo root `lingxi-code/`; git from repo root, `lingxi-code/...` paths, **NEVER `git add -A`**. `-D missing-docs` + clippy pedantic. The new bin crate may locally `#![allow(unsafe_code)]` IF needed (the seccomp/exec path) — prefer `nix`/`seccompiler` safe wrappers; document any unsafe. The `sandbox-runtime` lib keeps `#![forbid(unsafe_code)]`. Footer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. Docker for the e2e.

---

### Task 1: the `apply-seccomp` binary crate

**Files:** Create `lingxi-code/apply-seccomp/Cargo.toml` + `src/main.rs`; add to workspace `members` (NOT default-members if it shouldn't build on macOS — gate the deps by target). Modify root `Cargo.toml`.

- [ ] **`apply-seccomp` bin** (Linux): `main()` reads `argv[1..]` = the command + args to exec (e.g. `bash -c "<script>"`). Steps:
  1. `nix::sys::prctl::set_no_new_privs()` (or libc `PR_SET_NO_NEW_PRIVS`).
  2. Build + install a seccomp filter with `seccompiler`: default action `Allow`; rule on the `socket` syscall (`libc::SYS_socket`) — when arg0 (domain) `== AF_UNIX` (libc `AF_UNIX` = 1) → `Errno(EPERM)`; same for `socketpair` (arg0 == AF_UNIX). Compile to `BpfProgram` for the target arch (seccompiler `TargetArch::x86_64`/`aarch64` — detect at compile time via `cfg!(target_arch)`); `seccompiler::apply_filter(&prog)`. If the arch is unsupported (not x86_64/aarch64) → exit with an error (matching the TS ia32-unsupported stance).
  3. `nix::unistd::execvp(&argv[1], &argv[1..])` — replaces the process; on error exit non-zero.
  On non-Linux targets the crate builds an empty `main` that errors "apply-seccomp is Linux-only" (so the workspace still builds on macOS).
- [ ] **Tests** (the crate is mostly a thin syscall wrapper — unit-test what's pure): a `build_seccomp_filter()` fn factored out returning the `seccompiler::SeccompFilter` (or the compiled `BpfProgram`) — assert it compiles for x86_64 + aarch64 without error and has the socket/socketpair rules. The actual EPERM behavior is Docker-verified (Task 3). Commit (`feat(apply-seccomp): seccompiler AF_UNIX socket block + exec (P7)`).

### Task 2: wire resolve_apply_seccomp_prefix + build_sandbox_command

- [ ] In `linux.rs`, replace the P4-2c stub `resolve_apply_seccomp_prefix() -> Option<String>` with a faithful locator (port `getApplySeccompBinaryPath` shape): `resolve_apply_seccomp_prefix(apply_path: Option<&Path>, argv0: Option<&str>) -> Option<String>` — explicit `apply_path` (if it exists) → its path; else search for the built `apply-seccomp` binary (a configurable/relative path — for the LingXi build it's the workspace `target/<profile>/apply-seccomp`, or a path passed via the seccomp config); `argv0` mode → trust it resolves inside bwrap. Return the prefix string = `<binary-path> ` (a space-terminated prefix prepended before the shell command, matching `applySeccompPrefix + shellquote([shell, -c, cmd])`). Arch gate: return `None` for non-x64/arm64 (TS parity).
- [ ] Thread it into `wrap_command_with_sandbox_linux`: when `!allow_all_unix_sockets`, `apply_seccomp_prefix = resolve_apply_seccomp_prefix(seccomp_config…)`; pass it to `build_sandbox_command` (the seccomp branch — already ported, currently dead — becomes live) and the non-network `applySeccompPrefix` branch. When `allow_all_unix_sockets` → `None` (skip). Update `get_linux_dependency_status`/`check_linux_dependencies` `has_seccomp_apply` to reflect the real locator.
- [ ] Tests: `resolve_apply_seccomp_prefix` returns the explicit path when it exists, `None` when absent + no argv0, `None` on unsupported arch (simulate); `build_sandbox_command` with a seccomp prefix emits the `<prefix> bash -c '<cmd>'` branch (not the `eval` branch); `wrap_command_with_sandbox_linux` with `allow_all_unix_sockets=false` + a resolvable prefix includes it, `=true` omits it. Commit (`feat(sandbox-runtime): wire apply-seccomp prefix into wrap/build_sandbox_command (P7)`).

### Task 3: Docker e2e + gates

- [ ] Add a `seccomp` group to `scripts/verify-bwrap.sh`: build the `apply-seccomp` binary (in the container, or copy a linux build in), run a bwrap child wrapped with `apply-seccomp` that runs a tiny probe (a C one-liner or python: `socket(AF_UNIX, SOCK_STREAM)` should fail EPERM/EACCES; `socket(AF_INET, SOCK_STREAM)` should succeed). Assert `UNIX_BLOCKED` + `INET_OK`. (If building the Rust bin in-container is impractical due to the LinuxKit OOM seen in P4-2c/P5, document that + provide the probe + the bwrap+apply-seccomp invocation shape so it's runnable where resources allow; the seccompiler filter is unit-tested for compilation.)
- [ ] Final gates: `cargo test -p sandbox-runtime` + `cargo test -p apply-seccomp` (Linux) / build-check on macOS + clippy `-D warnings` on both + `cargo test --workspace --no-run` + `cargo tree -p engine-mobile -e normal | grep -c sandbox-runtime` (0) + `| grep -c apply-seccomp` (0) + frozen diff empty. Stage ONLY explicit paths.

## Final verification
1. The seccompiler filter blocks `socket(AF_UNIX)` + `socketpair(AF_UNIX)`, allows AF_INET; compiles for x86_64 + aarch64; ia32/other → unsupported (TS parity).
2. `resolve_apply_seccomp_prefix` faithful (explicit/argv0/arch-gate); the seccomp branch of build_sandbox_command/wrap is now LIVE; `allow_all_unix_sockets` skips it.
3. Docker (where runnable): in-sandbox AF_UNIX → EPERM, AF_INET → OK. Documented divergence: nested-ns/PID1-reaper provided by bwrap args; the AF_UNIX block is the faithful security-essential port (C source unavailable).
4. engine-mobile pulls 0 sandbox-runtime AND 0 apply-seccomp; frozen empty. macOS workspace still builds.
