# sandbox-runtime P9a — macOS Seatbelt backend (macos-sandbox-utils.js)

> REQUIRED SUB-SKILL: superpowers:subagent-driven-development.

**Goal:** Port `macos-sandbox-utils.js` into `sandbox-runtime/src/macos.rs` — the macOS Seatbelt (SBPL) sandbox backend: `generate_sandbox_profile` (the full `(version 1)` deny-default profile + read/write/network/unix-socket/mach/sysctl rules), the rule generators, `escape_path`, `wrap_command_with_sandbox_macos` (invokes `sandbox-exec -p <profile>`), `mac_get_mandatory_deny_patterns`, and `start_macos_sandbox_log_monitor`. RUNTIME-VERIFIABLE on this macOS host via `sandbox-exec`.

**Reference of truth:** `docs/superpowers/references/sandbox-runtime-0.0.54/dist/sandbox/macos-sandbox-utils.js` — READ THE ENTIRE FILE (679 lines). The profile is a large **verbatim** SBPL template; reproduce the static rules byte-for-byte (the `(version 1)`, `(deny default ...)`, the Chrome-derived process/mach/iokit/sysctl allowlists, the conditional sections) and the dynamic rules (read/write/network from the configs). Reuses `path_utils::{glob_to_regex, normalize_path_for_sandbox, contains_glob_chars, remove_trailing_glob_suffix, get_default_write_paths}` (P4-2a), `config::FilesystemConfig`, `fs_args::{ReadConfig, WriteConfig}`, `env::generate_proxy_env_vars` (P4-1, for the macOS env).

**Branch:** `parity-sandbox-runtime-p9a`. **Conventions:** Cargo root `lingxi-code/`; git from repo root, `lingxi-code/...` paths, **NEVER `git add -A`**. `-D missing-docs` + clippy pedantic. `#![forbid(unsafe_code)]`. macOS-gate the `sandbox-exec`-invoking code + tests (`#[cfg(target_os="macos")]`); the pure profile-text generation is portable + unit-testable everywhere. Footer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. This host IS macOS — the runtime gate runs.

---

### Task 1: rule generators + escape_path + mandatory-deny

**Files:** Create `lingxi-code/sandbox-runtime/src/macos.rs`; Modify `src/lib.rs`.

- [ ] Port `escape_path` (the SBPL string escaper — READ its exact behavior: quoting + escaping for `(literal "...")`/`(regex #"...")`/`(subpath "...")`), `mac_get_mandatory_deny_patterns(allow_git_config)` (the dangerous-files/dirs → regex patterns, dedup), and the rule generators: read rules (`generateFilesystemReadRules` — `(allow file-read*)` default, then `(deny file-read* ...)` for denyOnly [regex for globs / subpath for dirs], then `(allow file-read* ...)` re-allow for allowWithinDeny, + the metadata/ancestor rules), write rules (`(allow file-write*)` default OR allow-only + mandatory-deny + `generateMoveBlockingRules`), and the deny-op helpers. Faithful to the exact rule shapes + ordering (later-rule-wins Seatbelt semantics).
- [ ] Tests: `escape_path` on a path with quotes/spaces/special chars matches the TS; read rules for `{denyOnly:["/x"], allowWithinDeny:["/x/y"]}` → `(allow file-read*)` then `(deny file-read* (subpath "/x") ...)` then `(allow file-read* (subpath "/x/y") ...)` in that order; write rules for `{allowOnly:["/w"], denyWithinAllow:["/w/d"]}` → the allow + the deny + the mandatory-deny patterns; a glob path → `(... (regex ...))`. Commit (`feat(sandbox-runtime): macOS SBPL rule generators + escape_path + mandatory-deny (P9a)`).

### Task 2: generate_sandbox_profile + wrap + log monitor

- [ ] Port `generate_sandbox_profile(params)` (macos-sandbox-utils.js:260-end) — the FULL profile: the verbatim static header (version 1, deny-default-with-logTag, process-exec/fork/info, mach-lookup allowlist [the exact global-names], the conditional `enable_weaker_network_isolation` trustd.agent block, the `allow_apple_events` block, the `allow_mach_lookup` user services [trailing-`*` → `global-name-prefix`], ipc-posix-shm/sem, iokit, system-socket AF_SYSTEM, the full sysctl-read allowlist, the file-ioctl/pty block when `allow_pty`), then the read rules + write rules, then the NETWORK section: `needs_network_restriction` false → `(allow network*)`; true → the unix-socket rules (`allow_all_unix_sockets` → AF_UNIX + path-regex `^/`; else `allow_unix_sockets` subpaths) + `allow_local_binding` (network-bind/inbound/outbound local ip `*:*`) + the proxy-port allows (`localhost:<httpProxyPort>`/`<socksProxyPort>` bind/inbound/outbound) when ports set. Reproduce EXACTLY (byte-faithful static text + the dynamic rules).
- [ ] Port `wrap_command_with_sandbox_macos(params) -> String` — builds the profile, writes it to a temp `.sb` file (or passes via `-p`), returns the `sandbox-exec -f <profile> /bin/sh -c <command>` (or `-p` inline) invocation with the proxy env vars (`generate_proxy_env_vars(..., Platform::Macos, tmpdir)`) prepended. Match the TS invocation shape (READ it — how it passes the profile + env + command).
- [ ] Port `start_macos_sandbox_log_monitor` (the violation log reader → SandboxViolationStore) — faithful; if it shells `log stream`, keep it macOS-gated + document.
- [ ] **Runtime gate (macOS, `#[cfg(target_os="macos")]` test):** (a) `generate_sandbox_profile` output is accepted by `sandbox-exec` — `sandbox-exec -p '<profile>' /usr/bin/true` exits 0 (the profile compiles); (b) a write-deny actually blocks — a profile denying writes to a temp dir → `sandbox-exec -p '<profile>' /bin/sh -c 'echo x > <denied>/f'` FAILS; (c) a read-allow within deny works. Use real `std::process::Command` on `sandbox-exec`. Commit (`feat(sandbox-runtime): macOS generate_sandbox_profile + wrap + log monitor + sandbox-exec runtime gate (P9a)`).

### Task 3: gates
`cargo test -p sandbox-runtime` (incl. the macOS runtime tests on this host) + clippy `-D warnings` + `cargo test --workspace --no-run` + `cargo tree -p engine-mobile -e normal | grep -c sandbox-runtime` (0) + frozen diff empty. Stage ONLY explicit sandbox-runtime + Cargo paths.

## Final verification
1. The SBPL profile is byte-faithful to the TS (static header verbatim + dynamic read/write/network rules + the conditional sections); `escape_path` + the rule generators match.
2. **`sandbox-exec` accepts the generated profile AND a real deny blocks** (runtime-verified on this macOS host).
3. wrap_command_with_sandbox_macos builds the sandbox-exec invocation + proxy env; log monitor ported (macOS-gated).
4. engine-mobile 0-dep; frozen empty. Portable profile-text tests pass on all platforms; sandbox-exec tests macOS-gated.
