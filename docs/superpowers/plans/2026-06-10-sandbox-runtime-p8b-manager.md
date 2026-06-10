# sandbox-runtime P8b — SandboxManager orchestrator (sandbox-manager.js, Linux path)

> REQUIRED SUB-SKILL: superpowers:subagent-driven-development.

**Goal:** Port the `sandbox-manager.js` orchestrator into `sandbox-runtime/src/manager.rs` as a `SandboxManager` — the lifecycle/integration capstone that ties together config (P8a) → MITM CA (P6a) → HTTP/SOCKS proxies (P3b/P5) → Linux socat bridge (P4-2c) → `wrap_command_with_sandbox_linux` (P4-2c) → violation store (P8a), plus the per-request filter with an ask-callback. Linux is fully wired; macOS/Windows `wrap` branches are documented P9 seams.

**Reference of truth:** `docs/superpowers/references/sandbox-runtime-0.0.54/dist/sandbox/sandbox-manager.js` — READ it (esp. `filterNetworkRequest` :62-122, `startHttpProxyServer`/`startSocksProxyServer` :185-235, `initialize` :237-330, `wrapWithSandbox` :533-665, `updateConfig` :736-748, `reset`). Reuses: `matcher::{matches_domain_pattern, filter_network_request}`, `host::{is_valid_host, canonicalize_host}`, `mitm_ca::{create_mitm_ca, dispose_mitm_ca}`, `http_proxy::{serve, ProxyOptions}`, `socks_proxy::{serve_socks, SocksOptions}`, `linux::{initialize_linux_network_bridge, wrap_command_with_sandbox_linux, WrapParams, cleanup_bwrap_mount_points, check_linux_dependencies}`, `config::{SandboxRuntimeConfig, FilesystemConfig}`, `fs_args::{ReadConfig, WriteConfig}`, `path_utils::{remove_trailing_glob_suffix, contains_glob_chars, expand_glob_pattern, get_default_write_paths}`, `env::Platform`, `violation_store::SandboxViolationStore`.

**Branch:** `parity-sandbox-runtime-p8b`. **Conventions:** Cargo root `lingxi-code/`; git from repo root, `lingxi-code/...` paths, **NEVER `git add -A`**. `-D missing-docs` + clippy pedantic. `#![forbid(unsafe_code)]`. Footer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. (The TS uses module-level singleton state; the Rust port uses an owned `SandboxManager` struct — document this faithful refactor: no global mutable state.)

---

### Task 1: the ask-callback filter

**Files:** Modify `src/matcher.rs` (or a new `manager.rs` helper).

- [ ] Port `filterNetworkRequest` with the ASK path (sandbox-manager.js:62-122): `filter_network_request_with_ask(port, host, config, ask) -> bool` — `is_valid_host` reject → false; `canonicalize_host(host).unwrap_or(host)`; deny-first loop over `denied_domains`; allow loop over `allowed_domains`; **unmatched → if `ask` is Some, call it (a `Fn(host, port) -> bool` / boxed async) and return its result; on callback error → false; if `ask` is None → false.** (P1's `filter_network_request` is the no-ask version — keep it; this adds the ask path.) The `ask` callback type: `Option<Arc<dyn Fn(&str, u16) -> Pin<Box<dyn Future<Output=bool>+Send>> + Send + Sync>>`.
- [ ] Tests: denied→false, allowed→true, unmatched+no-ask→false, unmatched+ask-yes→true, unmatched+ask-no→false, ask-error→false, malformed-host→false (never asks). Commit (`feat(sandbox-runtime): filter_network_request_with_ask (manager ask-callback) (P8b)`).

### Task 2: SandboxManager — initialize + proxies + wrap + reset

**Files:** Create `src/manager.rs`; Modify `src/lib.rs`.

- [ ] **`SandboxManager`** struct owning: `config: Option<SandboxRuntimeConfig>`, `violation_store: SandboxViolationStore`, and the running state `Option<RunningState { http_port, socks_port, http_task: JoinHandle, socks_task: JoinHandle, bridge: Option<LinuxBridge>, mitm_ca: Option<Arc<MitmCa>>, http_socket_path, socks_socket_path }>`. Plus the ask-callback.
- [ ] **`initialize(&mut self, config, ask_callback, enable_log_monitor)`** (sandbox-manager.js:237-330): validate the config (`config.validate()`); if `network.tls_terminate` → `create_mitm_ca`; pick a port range (skip if external `http_proxy_port`/`socks_proxy_port` set — use those); `serve` the HTTP proxy (ProxyOptions: config's NetworkConfig, parent_proxy from config, filter_request, mitm_ca, upstream_ca) bound on a localhost port; `serve_socks` the SOCKS proxy; on Linux → `initialize_linux_network_bridge(http_port, socks_port, socat_path)`; store RunningState. (The proxy's filter must use `filter_network_request_with_ask` with the manager's config + ask — wire the `ProxyOptions.config` + a filter that consults the ask-callback; if ProxyOptions only takes a NetworkConfig, extend it to optionally carry the ask-callback, OR have the proxy call a filter fn — keep faithful: unmatched asks.) Document any ProxyOptions extension.
- [ ] **`wrap_with_sandbox(&self, command, bin_shell, custom_config, ...) -> Result<(String, Vec<PathBuf>)>`** (sandbox-manager.js:533-665, LINUX branch only):
  - `write_config`: `allow_only = get_default_write_paths() ++ strip_write_globs(allowWrite)`, `deny_within_allow = strip_write_globs(denyWrite)` where `strip_write_globs` = map `remove_trailing_glob_suffix` then drop entries that still `contains_glob_chars` (Linux).
  - `read_config`: `deny_only = expand(denyRead)`, `allow_within_deny = expand(allowRead) ++ mitm_ca.cert_path (if set)` where `expand` = for each path, `remove_trailing_glob_suffix`; if it still `contains_glob_chars` (Linux) → `expand_glob_pattern` else the stripped path.
  - `needs_network_restriction = network config present` (allowedDomains defined, even if empty = block-all).
  - call `wrap_command_with_sandbox_linux(WrapParams{ command, needs_network_restriction, http_socket_path/socks_socket_path (only if proxy running), http_proxy_port/socks_proxy_port, ca_cert_path = mitm_ca.cert_path, read_config, write_config, enable_weaker_nested_sandbox, allow_all_unix_sockets, bin_shell, ripgrep_cmd, mandatory_deny_search_depth, allow_git_config, seccomp config, bwrap_path, socat_path, cwd, platform })`. macOS/Windows → `Err("wrap_with_sandbox: macOS/Windows is P9")` (documented seam).
- [ ] **`update_config`** (replace config; per-request denied/allowed take effect via the live filter; structural changes need reset+init — faithful note), **`get_config`**, **`check_dependencies`** (Linux → `check_linux_dependencies`), **`reset(&mut self)`** (abort proxy tasks, teardown bridge [Drop], `dispose_mitm_ca`, `cleanup_bwrap_mount_points`, clear RunningState).
- [ ] **Integration test (Linux-gated where bridge/socat needed; the wrap-output assertions run anywhere):** `initialize` with `allowedDomains=["github.com"]` → the HTTP+SOCKS proxies are listening (assert ports bound); `wrap_with_sandbox("echo hi", "bash", None)` → the returned bwrap string contains `--unshare-net`, the socket binds, `--setenv HTTP_PROXY`, and the fs args + the sandbox socat command; a denied host through the running proxy → 403 / SOCKS REP_NOT_ALLOWED (reuse the proxy test pattern); `reset` tears down (ports no longer bound). The MITM-CA-readable: with `tlsTerminate` set, the wrap read_config allow_within_deny includes the CA cert path. Commit (`feat(sandbox-runtime): SandboxManager lifecycle — init/wrap/reset (Linux) (P8b)`).

### Task 3: gates
`cargo test -p sandbox-runtime` + clippy `-D warnings` + `cargo test --workspace --no-run` + `cargo tree -p engine-mobile -e normal | grep -c sandbox-runtime` (0) + frozen diff empty. Stage ONLY explicit sandbox-runtime + Cargo paths.

## Final verification
1. filter-with-ask faithful (deny-first, allow, unmatched→ask-or-deny, malformed→deny-never-ask).
2. Manager lifecycle: initialize builds CA+proxies+bridge; wrap_with_sandbox maps FilesystemConfig→Read/WriteConfig (default-write-paths, glob strip/expand, CA-cert-readable) + threads proxy sockets/ports/CA into wrap_command_with_sandbox_linux; reset tears everything down. No global state (owned struct).
3. macOS/Windows wrap = documented P9 seams.
4. engine-mobile 0-dep; frozen empty.
