# sandbox-runtime P6b — TLS-terminating proxy + CONNECT MITM wiring (tls-terminate-proxy.js)

> REQUIRED SUB-SKILL: superpowers:subagent-driven-development.

**Goal:** Port `tls-terminate-proxy.js` into `sandbox-runtime/src/tls_terminate.rs` and wire it into the CONNECT MITM seam of `http_proxy.rs`. When a `MitmCa` is configured, a CONNECT no longer opaque-tunnels: after the allowlist passes, the proxy writes `200`, sniffs the client's first bytes for a TLS ClientHello, and if it's TLS, terminates it with a per-host minted leaf (P6a), parses the decrypted HTTP/1.1, runs the `filterRequest` body hook on each request, and re-issues each upstream over a fresh real-TLS connection to the origin. Non-TLS CONNECT bytes fall through to the existing opaque tunnel. SECURITY-CRITICAL (this is the actual TLS interception path).

**Reference of truth:** `docs/superpowers/references/sandbox-runtime-0.0.54/dist/sandbox/tls-terminate-proxy.js` (`looksLikeClientHello`, `peekForClientHello`, `terminateAndForward`, `forwardUpstream`). Uses P6a `mitm_ca::MitmCa` + `mitm_leaf::{mint_leaf_cert, server_config_for}`, P3b `request_filter::decide_and_respond` + `parent_proxy::strip_hop_by_hop`.

**Deps:** `tokio-rustls` 0.25 (server + client), `rustls` 0.22, `hyper`/`hyper-util`/`http-body-util` (all in lock). For upstream TLS to the origin: rustls client with system roots (`rustls-native-certs` — check lock; else `webpki-roots` already a dep) + optional `upstream_ca`.

**Branch:** `parity-sandbox-runtime-p6b`. **Conventions:** Cargo root `lingxi-code/`; git from repo root, `lingxi-code/...` paths, **NEVER `git add -A`**. `-D missing-docs` + clippy pedantic. `#![forbid(unsafe_code)]`. Footer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

---

### Task 1: ClientHello sniff + an SNI cert-resolver

**Files:** Create `lingxi-code/sandbox-runtime/src/tls_terminate.rs`; add a `ResolvesServerCert` to `mitm_leaf.rs`; Modify `src/lib.rs`.

- [ ] **`looks_like_client_hello(buf: &[u8]) -> bool`** (tls-terminate-proxy.js:25-29): `buf.len() >= 3 && buf[0]==0x16 && buf[1]==0x03 && buf[2] <= 0x03`. Test: a real ClientHello prefix → true; `GET ` → false; `<3` bytes → false.
- [ ] **`peek_client_hello<S: AsyncRead+Unpin>(stream, already: Vec<u8>) -> (bool, Vec<u8>)`** — if `already.len()>=3` decide immediately; else read until ≥3 bytes (or EOF), return `(looks_like_client_hello(&buf), buf)` (the consumed bytes returned so the caller replays them into the TLS acceptor). Faithful to `peekForClientHello`.
- [ ] **`MitmCertResolver`** in `mitm_leaf.rs`: a `rustls::server::ResolvesServerCert` backed by `Arc<MitmCa>` + a default hostname; `resolve(client_hello)` → mint/cache an `Arc<rustls::sign::CertifiedKey>` for the SNI (or the default host if no SNI) via `mint_leaf_cert` (RSA leaf chain + key → `CertifiedKey`). Cache on the CA (add a `cert_keys: Mutex<HashMap<String, Arc<CertifiedKey>>>` or reuse a cache). Test: resolver returns a CertifiedKey whose leaf CN/SAN matches the SNI.

### Task 2: terminate_and_forward

- [ ] **`terminate_and_forward(ca, filter_request, client_stream, head, target: TlsTarget)`** (`TlsTarget { hostname, port, upstream_ca: Option<...> }`) — `terminateAndForward` + `forwardUpstream`:
  - Build a server `rustls::ServerConfig` with `cert_resolver = Arc<MitmCertResolver>` (SNI), `alpn_protocols=["http/1.1"]`. `tokio_rustls::TlsAcceptor::from(config).accept(prepended_stream)` where `prepended_stream` replays `head` then the client stream (use a chained reader).
  - Serve HTTP/1.1 over the decrypted TLS stream with hyper `http1::Builder::serve_connection` + a service that for each request:
    - run `decide_and_respond(filter_request, ...)` with the absolute URL `https://<host><path>` (`host` = the request Host header, or `hostname[:port]` when port≠443) when `filter_request` is set; on deny → 403 (request-filter's byte-exact 403); on allow → forward.
    - `strip_hop_by_hop` the request headers, **drop the Host header** (let the upstream client derive it), re-issue to the ORIGIN over a fresh tokio-rustls CLIENT connection: connect TCP `hostname:port`, TLS-handshake with system roots (+ `upstream_ca` appended if set), SNI=hostname (skip for IP literals), `agent: false` equivalent (a fresh connection per request — no pool). Relay the upstream response (status + `strip_hop_by_hop` headers + body) back over the decrypted stream. Upstream error → 502 "Bad Gateway".
  - On a TLS handshake error → log + close (the `tlsClientError` path). Document: WebSocket/upgrade over TLS is out of scope (the TS refuses it).
- [ ] **Integration test (real loopback, in-process):** stand up a stand-in HTTPS origin (a tokio-rustls server with its own self-signed cert, trusted via `upstream_ca`) returning 200 "ok". Drive `terminate_and_forward` with a tokio-rustls CLIENT that trusts the P6a CA. Assert: (a) the client's HTTPS request is terminated + forwarded and the "ok" body comes back (the MITM round-trips); (b) with a `filter_request` that denies a path → the client gets a 403 with `X-Proxy-Error: blocked-by-sandbox-runtime`; (c) the leaf the client received has SAN matching the host. Commit (`feat(sandbox-runtime): TLS-terminating MITM proxy + upstream re-issue (P6b)`).

### Task 3: wire into the CONNECT MITM seam

- [ ] Add to `ProxyOptions`: `mitm_ca: Option<Arc<MitmCa>>`, `tls_terminate_upstream_ca: Option<Arc<...>>` (the `target.upstreamCA`). Update the Debug impl.
- [ ] In the CONNECT handler (the `// P6:` seam): after the allowlist filter passes AND `mitm_ca` is set — reply `200 Connection Established`, `hyper::upgrade::on`, then `peek_client_hello` the upgraded stream; if TLS → `terminate_and_forward(ca, filter_request, upgraded, head, TlsTarget{hostname, port, upstream_ca})` (NO upstream dial here — terminate does its own per-request origin dials); if NOT TLS → fall through to the existing dial (parent/direct) + opaque `copy_bidirectional` with the peeked `head` prepended. When `mitm_ca` is `None` → the existing P3b path unchanged.
- [ ] **Integration test:** a full CONNECT-through-the-hyper-proxy with `mitm_ca` set: a tokio-rustls client (trusting the CA) does CONNECT + TLS + GET through the proxy to a stand-in origin → 200; assert a denied path → 403. Plus: `mitm_ca=None` → opaque tunnel still works (the P3b tests still pass). Commit (`feat(sandbox-runtime): wire TLS-MITM into the CONNECT handler (P6b)`).

### Task 4: gates (+ optional Docker)
- [ ] Optional `mitm` group in `scripts/verify-bwrap.sh` if a stand-in is cheap; else document the in-process integration tests are the runtime proof (the Rust proxy can't cross-compile into the container).
- [ ] Final gates: `cargo test -p sandbox-runtime` + clippy `-D warnings` + `cargo test --workspace --no-run` + `cargo tree -p engine-mobile -e normal | grep -c sandbox-runtime` (0) + frozen diff empty. Stage ONLY explicit sandbox-runtime + scripts + Cargo paths.

## Final verification
1. ClientHello sniff faithful; non-TLS CONNECT still opaque-tunnels (the SSH-over-CONNECT case).
2. terminate_and_forward: per-host leaf via SNI resolver, decrypt → filterRequest → re-issue upstream over REAL TLS (system roots + optional upstream_ca, SNI, no pool), relay; hop-by-hop stripped, Host dropped; 502 on upstream error.
3. The MITM round-trips in-process (CA-trusting client gets the origin's response) and filterRequest denials return the byte-exact 403. mitm_ca=None leaves the P3b opaque path unchanged.
4. engine-mobile 0-dep; frozen empty.
