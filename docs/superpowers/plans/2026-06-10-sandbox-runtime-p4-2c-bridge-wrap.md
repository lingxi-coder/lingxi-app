# sandbox-runtime P4-2c — socat bridge + wrapCommandWithSandboxLinux + Docker e2e (linux-sandbox-utils part 2)

> REQUIRED SUB-SKILL: superpowers:subagent-driven-development.

**Goal:** Finish `linux-sandbox-utils.js` — the dependency check, the socat netns bridge lifecycle, `buildSandboxCommand`, the full `wrapCommandWithSandboxLinux` bwrap argv assembly (threading P4-1 env + P4-2b fs-args), and mount-point cleanup. **This completes the minimal faithful base "socat domain filter" end-to-end** and is verified by a `--privileged` Docker bwrap-child test.

**Reference (READ — `docs/superpowers/references/sandbox-runtime-0.0.54/dist/sandbox/linux-sandbox-utils.js`):** `isExecutable` :285-296, `getLinuxDependencyStatus` :297-311, `checkLinuxDependencies` :312-363, `initializeLinuxNetworkBridge` :364-470, `resolveApplySeccompPrefix` :485-498, `buildSandboxCommand` :499-525, `wrapCommandWithSandboxLinux` :822-983, `cleanupBwrapMountPoints` :247-283. Uses P4-1 `env::generate_proxy_env_vars` + P4-2b `fs_args::generate_filesystem_args`. `shlex` (shell-quote) is in the lock.

**Scope note (seccomp is P7):** `resolve_apply_seccomp_prefix` returns `None` here (the apply-seccomp binary/BPF is P7); so the `apply_seccomp_prefix` is `None` and `build_sandbox_command` takes the no-seccomp branch (socat listeners + `eval`). Document the P7 seam. `allow_all_unix_sockets` path skips seccomp anyway.

**Branch:** `parity-sandbox-runtime-p4-2c`. **Conventions:** Cargo root `lingxi-code/`; git from repo root, `lingxi-code/...` paths, **NEVER `git add -A`**. `-D missing-docs` + clippy pedantic. `#![forbid(unsafe_code)]`. Footer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. Docker running for the e2e.

---

### Task 1: dependency check + cleanup + seccomp-prefix seam

**Files:** Create `lingxi-code/sandbox-runtime/src/linux.rs`; Modify `src/lib.rs`.

- [ ] Port `is_executable(p)` (access X_OK — use `std::fs::metadata` + unix permissions or a `which`-style check), `which_sync(cmd)` (PATH lookup — `which` crate is in lock; or std), `get_linux_dependency_status(opts) -> {has_bwrap, has_socat, has_seccomp_apply}`, `check_linux_dependencies(opts) -> {errors: Vec<String>, warnings: Vec<String>}` (bwrap/socat missing → errors with the EXACT messages `"bubblewrap (bwrap) not installed"` / `"socat not installed"` / the explicit-path-not-executable forms; seccomp-absent → warning `"seccomp not available - unix socket access not restricted"`). `resolve_apply_seccomp_prefix(...) -> Option<String>` returns `None` (P7 seam, documented). `cleanup_bwrap_mount_points(mount_points: &[PathBuf])` — for each: if it's an empty (size-0) file → unlink; if an empty dir → rmdir; ignore errors. (Faithful refactor: no global `activeSandboxCount`/`bwrapMountPoints` Set — the caller passes the `mount_points` returned by `generate_filesystem_args`; each invocation owns its cleanup. Document this.)
- [ ] Unit tests: dep-check messages (mock missing bwrap/socat via opts with a bogus explicit path); cleanup removes an empty tempfile + empty tempdir but leaves a non-empty one. Commit (`feat(sandbox-runtime): linux dependency check + mount-point cleanup + seccomp seam (P4-2c.1)`).

### Task 2: socat bridge lifecycle + buildSandboxCommand

- [ ] Port `initialize_linux_network_bridge(http_proxy_port, socks_proxy_port, socat_path) -> io::Result<LinuxBridge>` (:364-470): random 8-byte hex id; `http_socket_path`/`socks_socket_path` under `std::env::temp_dir()` as `claude-http-<id>.sock`/`claude-socks-<id>.sock`; spawn (tokio::process or std) two `socat UNIX-LISTEN:<sock>,fork,reuseaddr TCP:localhost:<port>,keepalive,keepidle=10,keepintvl=5,keepcnt=3` (EXACT argv); readiness poll (5 attempts, sleep i*100ms, check both socket files exist, error if a child died); on failure SIGTERM the started child(ren). `LinuxBridge { http_socket_path, socks_socket_path, http_child, socks_child, http_proxy_port, socks_proxy_port }` with a `teardown()`/`Drop` that SIGTERMs both children.
- [ ] Port `build_sandbox_command(http_sock, socks_sock, user_command, apply_seccomp_prefix: Option<&str>, shell, socat) -> String` (:499-525): shellquote the socat path; the inner script = `socat TCP-LISTEN:3128,fork,reuseaddr UNIX-CONNECT:<http_sock> >/dev/null 2>&1 &` + the :1080 socks line + `trap "kill %1 %2 2>/dev/null; exit" EXIT` + (seccomp-prefix present ? `<prefix> <shell -c user_command shellquoted>` : `eval <user_command shellquoted>`); return `<shell> -c <shellquote(inner_script)>`. Use `shlex::try_quote`/`shlex::join` for shellquote.
- [ ] Tests: the bridge socket paths + exact socat argv (assert the arg vector); `build_sandbox_command` byte-exact inner script shape (no-seccomp `eval` branch). The bridge SPAWN test is gated to skip if `socat` absent on the host (report). Commit (`feat(sandbox-runtime): socat netns bridge lifecycle + buildSandboxCommand (P4-2c.2)`).

### Task 3: wrapCommandWithSandboxLinux

- [ ] Port `wrap_command_with_sandbox_linux(params) -> io::Result<(String /*bwrap command*/, Vec<PathBuf> /*mount_points*/)>` (:822-983). `params` = `{ command, needs_network_restriction, http_socket_path, socks_socket_path, http_proxy_port, socks_proxy_port, ca_cert_path, read_config, write_config, enable_weaker_nested_sandbox, allow_all_unix_sockets, bin_shell, ripgrep_cmd, mandatory_deny_search_depth, allow_git_config, bwrap_path, socat_path, cwd, platform }`. Logic:
  - `has_read = read_config.deny_only non-empty`; `has_write = write_config.is_some()`; if `!needs_network && !has_read && !has_write` → return `(command, vec![])`.
  - `bwrap_args = ["--new-session", "--die-with-parent"]`. seccomp prefix = `None` (P7).
  - if needs_network: `--unshare-net`; if both sockets present → `--bind <http_sock> <http_sock>`, `--bind <socks_sock> <socks_sock>`, then `generate_proxy_env_vars(http_port, socks_port, ca_cert_path, platform, tmpdir)` → for each `(k,v)`: `--setenv k v`; + `--setenv CLAUDE_CODE_HOST_HTTP_PROXY_PORT <p>` / `..._SOCKS_..._PORT <p>` (when set). (No sockets → just `--unshare-net`, full block.)
  - `fs_args = generate_filesystem_args(read_config, write_config, ripgrep_cmd, depth, allow_git_config, cwd)` → push args, collect mount_points. Then `--dev /dev`; `--unshare-pid`; if `!enable_weaker_nested` → `--proc /proc` else `--unshare-user --bind /proc /proc`.
  - shell = which(bin_shell || "bash") (err if not found); `-- <shell> -c <cmd>` where cmd = (needs_network && both sockets) ? `build_sandbox_command(...)` : (seccomp ? ... : command). Final = `shlex` quote of `[bwrap_path||"bwrap", ...bwrap_args]`.
  - Return `(wrapped, mount_points)`.
- [ ] Tests (tempdir): (a) no restrictions → returns command unchanged + empty mount_points; (b) network-only → `--unshare-net` + socket binds + `--setenv HTTP_PROXY http://localhost:3128` + the sandbox socat command; (c) write-restrict → fs_args present + `--unshare-pid` + `--proc /proc`; (d) weaker-nested → `--unshare-user --bind /proc /proc`; (e) mount_points threaded from fs_args. Commit (`feat(sandbox-runtime): wrapCommandWithSandboxLinux full bwrap argv assembly (P4-2c.3)`).

### Task 4: Docker e2e + final gates

- [ ] Add a `netbridge` group to `scripts/verify-bwrap.sh` proving the bridge mechanism end-to-end in a `--privileged arm64v8/debian` container (install bubblewrap + socat + curl + a stand-in HTTP CONNECT proxy — e.g. `tinyproxy` or a 20-line python `http.server`-based CONNECT proxy that only allows one host): start the stand-in proxy on the host; start the host socat `UNIX-LISTEN:<sock> TCP:localhost:<proxyport>`; run a bwrap child = `--unshare-net --bind <sock> <sock> --ro-bind / / --proc /proc --dev /dev --unshare-pid --setenv HTTP_PROXY http://localhost:3128 -- bash -c '<sandbox socat listener>; curl -sS https://<allowed> && echo NET_OK || echo NET_FAIL'`; assert the allowed host is reachable THROUGH the bridge (NET_OK) and a DIRECT curl (no proxy, fresh netns) is blocked (NET_BLOCKED). This mirrors the EXACT shape `wrap_command_with_sandbox_linux` emits (the Rust unit tests pin that shape). Document that the in-container proxy is a stand-in for the Rust hyper proxy (P3b, separately tested) — the e2e proves the socat/bwrap/env PLUMBING.
- [ ] `scripts/verify-bwrap.sh netbridge` → ALL-PASS. Report output.
- [ ] Final gates: `cargo test -p sandbox-runtime` + clippy `-D warnings` + `cargo test --workspace --no-run` + `cargo tree -p engine-mobile -e normal | grep -c sandbox-runtime` (0) + frozen diff empty. Stage ONLY explicit sandbox-runtime + scripts + Cargo paths.

## Final verification
1. `wrapCommandWithSandboxLinux` ported branch-for-branch (the no-restriction short-circuit, --unshare-net+binds+setenv, fs_args, --proc vs weaker-nested, the shell command via build_sandbox_command). mount_points threaded for cleanup.
2. The socat argv + env shape byte-exact; the bridge lifecycle spawns + tears down socat.
3. **Docker `netbridge` proves the child reaches an allowed host through the bridge while direct egress is blocked.** This completes the base socat-domain-filter e2e.
4. engine-mobile 0-dep; frozen empty.
