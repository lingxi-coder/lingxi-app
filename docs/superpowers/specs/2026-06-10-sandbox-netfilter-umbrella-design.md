# Sandbox Network Filtering (socat domain filter) — Umbrella Design

**Date:** 2026-06-10
**Status:** Approved scope: FULL faithful port (user). Decomposed into 7 dependency-ordered, independently-shippable sub-projects; each its own spec→plan→execute→Docker-verify cycle.
**Reference of truth:** the vendored `@anthropic-ai/sandbox-runtime@0.0.54` JS at `docs/superpowers/references/sandbox-runtime-0.0.54/dist/sandbox/` (the mechanism is NOT in claude-code/src). Re-fetch: `npm pack @anthropic-ai/sandbox-runtime@0.0.54`.
**Builds on:** the merged Linux bwrap hardening (`ef4df023`) — this replaces the conservative "allowed_domains → no-net" interim with the real per-domain proxy+bridge filtering.
**Related memory:** `[[bwrap-docker-verification]]` (Docker harness), `[[parity-1to1-effort]]`.

## What this actually is

NOT "a socat companion process." The mechanism (per `linux-sandbox-utils.js:338-523`, `sandbox-manager.js`): on Linux, `bwrap --unshare-net` makes the netns all-or-nothing. Domain filtering happens in a **host-side forward-proxy stack** the sandboxed process is forced to use via injected `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY` env. Traffic crosses the netns boundary via **two socat hops over a Unix socket** (sandbox-side `TCP-LISTEN:3128/1080` → bound Unix socket → host-side `UNIX-LISTEN` → host TCP proxy). The host stack = an HTTP forward proxy (CONNECT + plain HTTP) + a SOCKS5 proxy (SSH/git) + an OPTIONAL TLS-MITM layer (CA + per-host leaf certs) for deep inspection. The allow/deny brain is `filterNetworkRequest`/`matchesDomainPattern` in `sandbox-manager.js` (hostname+port; deny-precedes-allow; empty allowlist = deny-all). seccomp blocks `socket(AF_UNIX)` so user commands can't bypass the bridge.

## Locked semantics (porter watch-items)

- **Matcher** (`sandbox-manager.js:46-119`): `*.example.com` ⇒ `host.ends_with(".example.com")` (so `a.example.com` matches, bare `example.com` does NOT); non-wildcard ⇒ exact case-insensitive equality; wildcards never match IP literals. **deny-precedes-allow**; unmatched ⇒ ask-callback else **deny**. **Empty allowedDomains = deny-all** (NOT allow-all).
- **Pattern grammar** (`sandbox-config.js:11-41`): `localhost` | `*.dom.tld` (≥2 labels after `*.`; `*.com`/`*` rejected) | exact `dom.tld` (must contain a dot). No protocol/path/port in patterns.
- **canonicalizeHost / isValidHost** (`parent-proxy.js:372,396`): collapse inet_aton shorthand + hex/octal octets + IPv6 compression + trailing dots; reject control chars / CRLF / zone IDs. SECURITY-critical (denylist-evasion + header-injection).
- **MITM is OPTIONAL**: base path = CONNECT-hostname allowlist + opaque tunnel (no TLS interception, no cert minting). MITM (`mitmCA` set) terminates TLS with a minted leaf and filters on full URL.
- **`allowManagedDomainsOnly` does NOT exist in 0.0.54** — do not port it.
- **Exact wire** (`linux-sandbox-utils.js:367-397`, `sandbox-utils.js:272-372`): host socat `UNIX-LISTEN:<sock>,fork,reuseaddr TCP:localhost:<port>,keepalive,keepidle=10,keepintvl=5,keepcnt=3`; sandbox socat `TCP-LISTEN:{3128,1080},fork,reuseaddr UNIX-CONNECT:<sock>`; ports 3128 (HTTP) / 1080 (SOCKS) hardcoded; the full env list (`HTTP(S)_PROXY=http://localhost:3128`, `ALL_PROXY=socks5h://localhost:1080`, the `NO_PROXY` set, `GIT_SSH_COMMAND` socat-PROXY, the `CA_TRUST_VARS` when MITM).

## LingXi integration

The current Rust sandbox (`sandbox/src/runtime_config.rs`, `platforms/posix/src/sandbox.rs`) has `NetworkPolicy {Disabled, LoopbackOnly, Allowed}` + `NetworkRestrictionConfig.allowed_domains`, with the just-merged conservative mapping routing any non-full-allow domain policy to `--unshare-net` no-net. This subsystem adds the REAL path: when a policy carries specific `allowed_domains`, the sandbox spins up the host proxy stack + socat bridge + env injection instead of no-net. Net new: companion-process lifecycle tied to the sandboxed command (the current `wrap_with_sandbox` produces a pure shell string; the proxy/bridge needs a managed lifecycle — likely a new `sandbox::netfilter` module owning the proxy servers + socat children, wired through `platforms/posix` `prepare`/run). Frozen `traits/`+`protocol/` stay untouched (additive sandbox-crate types only). engine-mobile must pull none of it.

## The 7 sub-projects (dependency-ordered; each: own spec/plan/execute/Docker-verify)

- **P1 — net-config + domain matcher** (pure, foundational). Port the pattern grammar validation, `matches_domain_pattern`, `filter_network_request` (deny-first/allow/canonicalize/empty=deny-all), the network config schema. Deps: serde, url, ipnet. Verify: table-driven unit tests (no bwrap).
- **P2 — host primitives** (pure-ish, security crux). `is_valid_host`, `canonicalize_host`, `strip_brackets`, NO_PROXY parse + `should_bypass_parent_proxy` (suffix + CIDR), parent-proxy URL selection. Deps: url, ipnet, idna. Verify: unit tests (CRLF/zone-ID/null-byte rejection + inet_aton/IPv6 canonicalization).
- **P3 — base HTTP CONNECT-allowlist proxy** (first runnable). `create_http_proxy_server` CONNECT + plain-HTTP, opaque tunnel, dial-direct/parent-proxy CONNECT, strip-hop-by-hop. NO MITM. Deps: tokio (hand-rolled CONNECT, no hyper/rustls needed). Verify (Docker): `curl -x http://127.0.0.1:PORT https://allowed` ok, `https://denied` blocked.
- **P4 — socat netns bridge + bwrap orchestration** (completes the base "socat domain filter" e2e). `initialize_linux_network_bridge` (exact host socat argv), sandbox-side 3128/1080 listeners + trap, `generate_proxy_env_vars` (exact env), `--unshare-net`/`--bind`/`--setenv` argv assembly, teardown. Deps: tokio::process, tempfile, rand, shell-quote. Verify (privileged Docker): bwrap child egresses to allowed host through the proxy; denied blocked; empty allowlist = no leak.
- **P5 — SOCKS5 + SSH/git** . `create_socks_proxy_server`, SOCKS5 DOMAINNAME validator → filter, opaque tunnel + parent-proxy, `GIT_SSH_COMMAND` socat-PROXY env. Deps: fast-socks5/tokio. Verify (Docker): git/ssh through bridge to allowed host ok; denied blocked.
- **P6 — TLS-MITM** (heaviest, security-critical, intentionally late). `create_mitm_ca`/`mint_leaf_cert` (RSA-2048+SHA-256, SAN-only, no-AKI, 99d clamp), ClientHello sniff, `terminate_and_forward`, `decide_and_respond` body hook + `filter_request`, CA-trust env + allow_read of the cert. Deps: rustls, tokio-rustls, rcgen, rustls-pemfile, x509-parser. Verify (Docker): curl with the CA-trust env hits an allowed host via a minted leaf; filter sees full URL + can 403 a denied path; non-TLS CONNECT falls through.
- **P7 — seccomp** (defense-in-depth, last). `socket(AF_UNIX)`-blocking via the vendored `apply-seccomp` binary or `seccompiler`; `allow_all_unix_sockets` bypass. Deps: seccompiler/nix/libc. Verify (Docker): in-sandbox `socket(AF_UNIX)` → EPERM while socat (started pre-seccomp) keeps the bridge alive; filtering still holds.

**First useful milestone = P3+P4** (the minimal faithful base socat domain filter). P5/P6/P7 extend it.

## Per-sub-project gate (every batch)

TDD; deterministic unit tests on macOS; the relevant `scripts/verify-bwrap.sh`-style Docker assertion (extend the harness per sub-project) as the runtime gate; `cargo test` + `clippy -D warnings` on touched crates; `cargo test --workspace --no-run` struct-trap; frozen `traits/protocol` empty diff; engine-mobile pulls none of the new net-filter code. New deps added per-sub-project (not all up front).

## Scope: FULL 1:1 of the entire package (user-clarified 2026-06-10)

The goal is a **complete behavioral 1:1 Rust rewrite of every file in
`@anthropic-ai/sandbox-runtime@0.0.54`** — not a partial/Linux-only subset.
The 7-phase ordering below is the BUILD SEQUENCE, not a scope boundary: every
component the package ships gets ported. Nothing is "out of scope" except
artifacts that are faithfully reproduced by a Rust equivalent (noted inline).

**Complete file-coverage map (every `dist/sandbox/*.js` + `dist/*.js` + `dist/utils/*.js`):**
- `sandbox-config.js` / `sandbox-schemas.js` → P1 (config + the full zod schema surface:
  `NetworkConfigSchema`, `domainPatternSchema`, `MitmProxyConfigSchema`,
  `ParentProxyConfigSchema`, `SeccompConfigSchema`, `SandboxRuntimeConfigSchema`,
  filesystem path schemas). P1 did the network subset; the remaining schemas are pending.
- `sandbox-manager.js` (matcher + full orchestration/lifecycle) → matcher in P1; the
  manager (proxy startup, MITM CA build, bridge start, `wrapWithSandbox`, `reset`,
  `updateConfig`, the ask-callback) is its own late integration phase.
- `parent-proxy.js` (FULL: resolve/NO_PROXY done in P2; PENDING: `openConnectTunnel`,
  `connectViaParentProxy`, `proxyAuthHeader`, `stripHopByHop`, `redactUrl`).
- `http-proxy.js` (FULL: CONNECT base done in P3; PENDING: plain-HTTP full-URI forwarding,
  parent-proxy routing, MITM routing, the `filterRequest` body integration).
- `request-filter.js` (the body-tee `decideAndRespond` hook) → with the proxy completion.
- `socks-proxy.js` (full SOCKS5) → P5.
- `mitm-ca.js` / `mitm-leaf.js` / `tls-terminate-proxy.js` (full MITM) → P6.
- `generate-seccomp-filter.js` (the `socket(AF_UNIX)` BPF — reimplement via `seccompiler`,
  NOT shell out to the vendored binary; that's the faithful Rust equivalent) → P7.
- `linux-sandbox-utils.js` (FULL: bridge + bwrap argv + mount-point cleanup + dep check +
  `wrapCommandWithSandboxLinux` + `buildSandboxCommand`) → P4.
- `sandbox-utils.js` (`generateProxyEnvVars`, the env/CA-trust var lists) → P4.
- `macos-sandbox-utils.js` (the FULL SBPL profile generation incl. the network rules) → a
  macOS phase. NOT out of scope — full 1:1. (The merged bwrap hardening only touched the
  existing simplified SBPL; the faithful `macos-sandbox-utils` SBPL is a port target.)
- `windows-sandbox-utils.js` (the Windows AppContainer/job-object sandbox) → a Windows phase.
- `sandbox-violation-store.js` (the ring-buffer + pub/sub) → small standalone port.
- `cli.js` (`srt` CLI) + `index.js` (the public API surface) → a final integration phase.
- `utils/{ripgrep,debug,which,platform,config-loader}.js` → ported as needed by consumers
  (some already have LingXi equivalents — reuse where behaviorally identical, port where not).

**Genuinely-faithful equivalents (not omissions):** HTTP/2 to the sandbox is faithfully
http/1.1-forced (the package's own ALPN choice); the pre-built `apply-seccomp` binary blob is
replaced by an in-Rust BPF (`seccompiler`) producing the same `socket(AF_UNIX)`-block (behavioral 1:1);
`node-forge`/Node `crypto` → `rcgen`/`rustls` producing wire-equivalent certs.

**Updated phase list (build order; each phase finishes ALL of its files' behavior):** P1 net-config+
matcher (done) → P2 parent-proxy resolve/NO_PROXY (done) → P3 base CONNECT (done) → **P3b proxy
completion** (plain-HTTP forwarding + parent-proxy routing + `openConnectTunnel`/`connectViaParentProxy`/
`proxyAuthHeader`/`stripHopByHop`/`redactUrl` + `request-filter` body hook) → P4 socat bridge + bwrap +
`sandbox-utils` env + `linux-sandbox-utils` full (Docker e2e) → P5 SOCKS5 → P6 MITM (ca/leaf/terminate) →
P7 seccomp (seccompiler) → P8 sandbox-manager orchestration + violation-store + remaining schemas → P9
macOS SBPL full + Windows sandbox → P10 CLI + public API. Every phase: full file behavior, no deferred
remnants.
