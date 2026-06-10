# sandbox-runtime P11 — close the two 1:1 gaps (Windows install flow + live update_config)

> REQUIRED SUB-SKILL: superpowers:subagent-driven-development.

**Goal:** Close the two tracked gaps so the port is a *complete* 1:1: (1) the Windows admin install/uninstall flow from `windows-sandbox-utils.js` (the remaining exports `installWindowsSandbox`/`uninstallWindowsSandbox`/`deleteWindowsGroup`/`createWindowsGroup`/`createWindowsWfp`/`windowsInstallInstructions`), and (2) `update_config` must affect ALREADY-RUNNING proxies (the TS reads the shared module config per-request; the Rust proxies snapshot an `Arc<NetworkConfig>` at `serve()` time — make the live config shared so `update_config` is live).

**Reference of truth:** `docs/superpowers/references/sandbox-runtime-0.0.54/dist/sandbox/{windows-sandbox-utils.js (install fns :156-377), sandbox-manager.js (updateConfig + the per-request filterNetworkRequest reading the shared `config`)}`.

**Branch:** `parity-sandbox-runtime-p11-gaps`. **Conventions:** Cargo root `lingxi-code/`; git from repo root, `lingxi-code/...` paths, **NEVER `git add -A`**. `-D missing-docs` + clippy pedantic. `#![forbid(unsafe_code)]`. Footer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. No new deps (use std `RwLock`).

---

### Task 1: Windows install/uninstall flow (windows.rs)

**Files:** Modify `lingxi-code/sandbox-runtime/src/windows.rs` + the lib.rs re-exports.

- [ ] Port (Windows subprocess `#[cfg(target_os="windows")]`; the argv-building + the exit-code mapping + the result types portable + unit-tested):
  - `install_windows_sandbox(opts) -> Result<WindowsInstallResult>` (:156-204): argv `["install", ...group_ref_args, --user-sid?, --sublayer-guid?, --proxy-port-range "lo-hi"?, --force?]`; the EXIT-CODE CONTRACT: 0 ok → `{group, wfp}`; 10 → `{group, wfp, cancelled:true}`; 11 → err "group create failed"; 12 → err "WFP filter install failed"; 13 → err "already exist under this sublayer with different configuration … Pass force…"; else → err "install failed (exit N)". (`group`/`wfp` from `get_windows_group_status`/`get_windows_wfp_status`.)
  - `uninstall_windows_sandbox(opts) -> Result<UninstallResult>` (:214-230): argv `["uninstall", --sublayer-guid?]`; 10 → `{cancelled:true}`; 0 → `{}`; else err. (Does NOT delete the group — documented.)
  - `delete_windows_group(ref) -> Result<()>` (:233-246): `["group","delete",...group_ref]`; non-0 → err "requires elevation".
  - `create_windows_group(ref) -> Result<()>` (:248-266): `["group","create",...group_ref, --user-sid?]`; non-0 → err "requires elevation".
  - `create_windows_wfp(ref) -> Result<()>` (:268-290): `["wfp","install",...group_ref, --sublayer-guid?, --proxy-port-range?]`; non-0 → err "requires elevation".
  - `windows_install_instructions(ref, sublayer_guid, group_state) -> String` (:354-377): the human-readable install-steps text (READ + reproduce faithfully).
- [ ] Tests (portable): each fn's argv vector (with/without the optional flags, the proxy-port-range `lo-hi` format); the exit-code→Result mapping (feed each status code to a pure `map_install_status(status, out, opts)` helper → the right Ok/Err); `windows_install_instructions` text for a sample. Re-export the new fns from lib.rs (matching index.js). Commit (`feat(sandbox-runtime): Windows install/uninstall/group/WFP admin flow (P11)`).

### Task 2: live update_config (shared NetworkConfig)

**Files:** Modify `src/http_proxy.rs`, `src/socks_proxy.rs`, `src/manager.rs` (+ their tests).

- [ ] Introduce a shared live config: type `SharedNetworkConfig = Arc<std::sync::RwLock<Arc<NetworkConfig>>>` (a small newtype or alias, documented). Change `ProxyOptions.config` and `SocksOptions.config` to read the CURRENT config per-request: either (a) change the field type to `SharedNetworkConfig` + provide a `ProxyOptions::with_static_config(NetworkConfig)` constructor for the existing direct/test callers, OR (b) add an optional `live_config: Option<SharedNetworkConfig>` that overrides `config` per-request when present. Pick the cleaner one; the REQUIREMENT: the filter reads the up-to-date config on each request, and **no lock is held across an `.await`** (read+`.clone()` the inner `Arc<NetworkConfig>` into a local, release the guard, then call `filter_network_request_with_ask`).
- [ ] In `manager.rs`: store the `SharedNetworkConfig` in `RunningState` (or on the manager); `initialize` builds it from the config's network + passes it to both proxies; **`update_config` writes the new `network` into the shared handle** (`*shared.write().unwrap() = Arc::new(new.network.clone())`) so running proxies see it immediately (+ keep storing the full new config for `get_config`/next-init). Document that structural changes (ports, MITM, fs) still need reset+init — only the allow/deny domain lists are live (faithful to the TS, whose per-request filter only re-reads `config.network.allowed/deniedDomains`).
- [ ] Tests: a `serve`-ing proxy with `allowedDomains=[]` (deny-all) → a request to `github.com` is denied (403/REP_NOT_ALLOWED); then `update_config` with `allowedDomains=["github.com"]` → a NEW request to `github.com` is now allowed (tunnels) — proving the live update on a running proxy. Plus a unit test that the filter reads the swapped config. Keep the existing proxy tests green (update their `ProxyOptions` construction). Commit (`feat(sandbox-runtime): live update_config — running proxies read shared NetworkConfig (P11)`).

### Task 3: gates
`cargo test -p sandbox-runtime` + clippy `-D warnings` + `cargo test --workspace --no-run` + `cargo build -p sandbox-runtime --bin srt --features cli` + `cargo tree -p engine-mobile -e normal | grep -c sandbox-runtime` (0) + frozen diff empty. Stage ONLY explicit sandbox-runtime + Cargo paths.

## Final verification
1. Windows install flow: all 6 fns ported (argv + exit-code contract + instructions text) + re-exported; portable arg/mapping tests.
2. **Live update_config: a running proxy's allow/deny decision changes after `update_config` without reset** (tested); no lock-across-await; structural changes still need reset (documented).
3. The full `@anthropic-ai/sandbox-runtime` package is now ported 1:1 with NO remaining gaps.
4. engine-mobile 0-dep; frozen empty; `srt` bin still builds.
