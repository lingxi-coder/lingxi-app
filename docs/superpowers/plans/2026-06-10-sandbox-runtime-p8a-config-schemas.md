# sandbox-runtime P8a — config schemas + violation store (sandbox-config.js + sandbox-violation-store.js)

> REQUIRED SUB-SKILL: superpowers:subagent-driven-development.

**Goal:** Port the remaining configuration schemas (`sandbox-config.js`) as serde structs with `validate()` methods (the zod `.refine()` rules), and the in-memory `SandboxViolationStore` ring buffer (`sandbox-violation-store.js`). These are the typed config surface the `sandbox-manager` (P8b) consumes.

**Reference of truth:** `docs/superpowers/references/sandbox-runtime-0.0.54/dist/sandbox/{sandbox-config.js, sandbox-violation-store.js}` — READ both. P1 already has `NetworkConfig{allowed_domains,denied_domains}` + `is_valid_domain_pattern`; P4-2b has `ReadConfig/WriteConfig` (the DERIVED fs shapes — `FilesystemConfig` here is the USER-facing shape the manager maps from); P4-1 has `env`; `parent_proxy::ParentProxyConfig` exists (align/reuse).

**Branch:** `parity-sandbox-runtime-p8a`. **Conventions:** Cargo root `lingxi-code/`; git from repo root, `lingxi-code/...` paths, **NEVER `git add -A`**. `-D missing-docs` + clippy pedantic. `#![forbid(unsafe_code)]`. serde `rename_all="camelCase"`, all new fields `#[serde(default, skip_serializing_if=...)]` / `Option` so they're additive. Footer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

---

### Task 1: extend `NetworkConfig` + add the sub-configs (config.rs)

**Files:** Modify `lingxi-code/sandbox-runtime/src/config.rs`.

- [ ] **Extend `NetworkConfig`** with the optional fields from `NetworkConfigSchema` (sandbox-config.js:93-178) — all additive (keep `allowed_domains`/`denied_domains` as the first two): `allow_unix_sockets: Option<Vec<String>>`, `allow_all_unix_sockets: Option<bool>`, `allow_local_binding: Option<bool>`, `allow_mach_lookup: Option<Vec<String>>`, `http_proxy_port: Option<u16>`, `socks_proxy_port: Option<u16>`, `mitm_proxy: Option<MitmProxyConfig>`, `tls_terminate: Option<TlsTerminateConfig>`, `parent_proxy: Option<ParentProxyConfig>`. (`filterRequest` is a runtime callback, NOT serialized — represent as a non-serde `Option<FilterRequestFn>` on a separate runtime-options struct, or omit from the serde config + document; the proxy already has `filter_request` wired separately in P3b.)
- [ ] **`MitmProxyConfig`** { `socket_path: String`, `domains: Vec<String>` } — `validate()`: socketPath non-empty, domains non-empty + each `is_valid_domain_pattern`.
- [ ] **`TlsTerminateConfig`** { `ca_cert_path: Option<String>`, `ca_key_path: Option<String>` } — `validate()`: `ca_cert_path.is_some() == ca_key_path.is_some()` else error "caCertPath and caKeyPath must be provided together".
- [ ] **`FilesystemConfig`** { `deny_read: Vec<String>`, `allow_read: Option<Vec<String>>`, `allow_write: Vec<String>`, `deny_write: Vec<String>`, `allow_git_config: Option<bool>` } (sandbox-config.js:179-200; each path non-empty via `filesystemPathSchema`).
- [ ] **`RipgrepConfig`** { `command: String`, `args: Option<Vec<String>>`, `argv0: Option<String>` }.
- [ ] **`SeccompConfig`** { `apply_path: Option<String>`, `argv0: Option<String>` }.
- [ ] **`WindowsConfig`** { `group_name: String` (default "sandbox-runtime-net"), `group_sid: Option<String>`, `wfp_sublayer_guid: Option<String>`, `proxy_port_range: Option<(u16,u16)>` } — `validate()`: group_sid (if set) matches `^S-1-`; wfp_sublayer_guid (if set) is a UUID; proxy_port_range `lo<=hi && hi-lo<=64`.
- [ ] **`IgnoreViolationsConfig`** = `HashMap<String, Vec<String>>` (a type alias or newtype).
- [ ] **`SandboxRuntimeConfig`** { `network: NetworkConfig`, `filesystem: FilesystemConfig`, `ignore_violations: Option<IgnoreViolationsConfig>`, `enable_weaker_nested_sandbox: Option<bool>`, `enable_weaker_network_isolation: Option<bool>`, `allow_apple_events: Option<bool>`, `ripgrep: Option<RipgrepConfig>`, `mandatory_deny_search_depth: Option<u8>` (1..=10), `allow_pty: Option<bool>`, `seccomp: Option<SeccompConfig>`, `bwrap_path: Option<String>` (absolute), `socat_path: Option<String>` (absolute), `windows: Option<WindowsConfig>` } — `validate()` runs all sub-validations + `bwrap_path`/`socat_path` (if set) are absolute (`binaryPathSchema`) + `mandatory_deny_search_depth ∈ 1..=10` + network domain patterns + `allow_mach_lookup` entries (single trailing `*` only).
- [ ] **Tests:** serde round-trip a full camelCase JSON config → struct → back; each `validate()` rejects its bad case (mitm empty domains, tlsTerminate one-path, windows bad SID/range, non-absolute bwrap_path, depth out of range, bad mach-lookup wildcard) and accepts the good case; defaults (WindowsConfig.group_name). Commit (`feat(sandbox-runtime): full config schemas + validate() (P8a)`).

### Task 2: `violation_store.rs`

**Files:** Create `lingxi-code/sandbox-runtime/src/violation_store.rs`; Modify `src/lib.rs`.

- [ ] **`SandboxViolationStore`** (sandbox-violation-store.js): a ring buffer `max_size=100`; `add_violation` (push, `total_count += 1`, truncate to last 100, notify), `get_violations(limit: Option<usize>)`, `get_count`, `get_total_count`, `get_violations_for_command(cmd)` (filter by `encode_sandboxed_command(cmd)` from P4-1 `env` — the `encoded_command` field), `clear` (empties violations but NOT total_count, notify), `subscribe(listener)` (add + immediately call with current + return an unsubscribe handle). `Violation` struct with at least `encoded_command: String` + the fields the TS stores (port/host/path/etc. — match the violation shape used by the manager; keep it faithful to whatever fields appear). Use `Mutex` for the shared state + `Box<dyn Fn(&[Violation]) + Send>` listeners.
- [ ] **Tests:** add past 100 → only last 100 retained, `total_count` keeps growing; `get_violations_for_command` filters by encoded command; `clear` empties but keeps total; subscribe fires immediately + on add + unsubscribe stops it. Commit (`feat(sandbox-runtime): SandboxViolationStore ring buffer + pub/sub (P8a)`).

### Task 3: gates
`cargo test -p sandbox-runtime` + clippy `-D warnings` + `cargo test --workspace --no-run` + `cargo tree -p engine-mobile -e normal | grep -c sandbox-runtime` (0) + frozen diff empty. Stage ONLY explicit sandbox-runtime + (if touched) Cargo paths.

## Final verification
1. All schemas ported with faithful `validate()` (the zod refines: domainPattern, binaryPath-absolute, tlsTerminate-together, windows SID/UUID/range, mach-lookup-trailing-*, depth 1..10); serde camelCase round-trips; NetworkConfig extensions are additive (matcher/proxy unaffected).
2. ViolationStore: ring(100), total-count-monotonic, per-command filter, clear-keeps-total, pub/sub.
3. engine-mobile 0-dep; frozen empty.
