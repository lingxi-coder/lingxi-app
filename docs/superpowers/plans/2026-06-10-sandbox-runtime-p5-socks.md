# sandbox-runtime P5 — SOCKS5 proxy (socks-proxy.js)

> REQUIRED SUB-SKILL: superpowers:subagent-driven-development.

**Goal:** Port `createSocksProxyServer` (socks-proxy.js) into `sandbox-runtime/src/socks_proxy.rs` — a hand-rolled RFC 1928 SOCKS5 server (no-auth) that validates the destination host (`is_valid_host` — the SOCKS5 DOMAINNAME is an unvalidated byte string), runs the allowlist filter BEFORE connecting, then routes the opaque TCP tunnel through the parent proxy (when set & not bypassed) or a direct dial. SECURITY-CRITICAL (enforces the allowlist for SSH/git/SOCKS traffic).

**Why hand-rolled:** the TS uses `@pondwader/socks5-server` whose ruleset-validator + custom-connection-handler hooks have no Rust equivalent; no socks crate is in the lock. SOCKS5 is a small, well-defined protocol — hand-rolling over tokio is faithful + dep-free and gives exact control over the pre-connect filter (the security boundary).

**Reference of truth:** `docs/superpowers/references/sandbox-runtime-0.0.54/dist/sandbox/socks-proxy.js`. Reuses `host::is_valid_host`, `matcher::filter_network_request`, `dial::dial_direct`, `parent_proxy::{connect_via_parent_proxy, should_bypass_parent_proxy, select_parent_proxy_url}` (all merged).

**Branch:** `parity-sandbox-runtime-p5`. **Conventions:** Cargo root `lingxi-code/`; git from repo root, `lingxi-code/...` paths, **NEVER `git add -A`**. `-D missing-docs` + clippy pedantic. `#![forbid(unsafe_code)]`. Footer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

---

### Task 1: SOCKS5 request parsing + reply codes

**Files:** Create `lingxi-code/sandbox-runtime/src/socks_proxy.rs`; Modify `src/lib.rs`.

- [ ] **Failing tests** for the wire parse/encode:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_connect_request_ipv4_domain_ipv6() {
        // VER=5 CMD=1 RSV=0 ATYP DST.ADDR DST.PORT
        // IPv4 1.2.3.4:443
        let v4 = [5,1,0,1, 1,2,3,4, 0x01,0xBB];
        assert_eq!(parse_request_target(&v4).unwrap(), ("1.2.3.4".to_string(), 443));
        // DOMAINNAME "example.com":80
        let mut d = vec![5,1,0,3, 11]; d.extend_from_slice(b"example.com"); d.extend_from_slice(&[0,80]);
        assert_eq!(parse_request_target(&d).unwrap(), ("example.com".to_string(), 80));
        // IPv6 ::1:8443
        let mut v6 = vec![5,1,0,4]; v6.extend_from_slice(&std::net::Ipv6Addr::LOCALHOST.octets()); v6.extend_from_slice(&[0x20,0xFB]);
        assert_eq!(parse_request_target(&v6).unwrap(), ("::1".to_string(), 8443));
        // bad CMD (not CONNECT) / bad VER → None
        assert!(parse_request_target(&[5,2,0,1,1,2,3,4,0,80]).is_none());
        assert!(parse_request_target(&[4,1,0,1,1,2,3,4,0,80]).is_none());
    }
}
```
- [ ] **Implement** `parse_request_target(buf) -> Option<(String,u16)>` (VER==5, CMD==1 CONNECT, ATYP 1/3/4; IPv4→dotted, DOMAINNAME→utf8 lossy of the length-prefixed bytes, IPv6→`Ipv6Addr` string; last 2 bytes = big-endian port) + the reply-code constants (`REP_GRANTED=0x00`, `REP_GENERAL_FAILURE=0x01`, `REP_NOT_ALLOWED=0x02`, `REP_HOST_UNREACHABLE=0x04`). Commit (`feat(sandbox-runtime): SOCKS5 request parse + reply codes (P5)`).

### Task 2: the SOCKS5 server

- [ ] **Implement** `serve_socks(listener: TcpListener, options: Arc<SocksOptions>)` (`SocksOptions { config: Arc<NetworkConfig>, parent_proxy: Option<Arc<ResolvedParentProxy>> }`). Per connection (own task):
  1. **Greeting**: read `VER NMETHODS METHODS[]`; if VER!=5 close; reply `05 00` (select NO-AUTH; if the client didn't offer 0x00, reply `05 FF` + close — faithful enough; the sandbox client always offers no-auth).
  2. **Request**: read the request; `parse_request_target`. If parse fails → reply `05 01 00 01 0.0.0.0:0` + close.
  3. **Validate** (the ruleset-validator, socks-proxy.js:6-33): `is_valid_host(host)` false → reply `REP_NOT_ALLOWED` + close; `filter_network_request(port, host, config)` false → reply `REP_NOT_ALLOWED` + close.
  4. **Dial** (the connection-handler, :37-78): `parent = parent_proxy && !should_bypass_parent_proxy(pp, host)` → `connect_via_parent_proxy(select_parent_proxy_url(pp, /*is_https*/ true), host, port)` (SOCKS is opaque ⇒ always prefer HTTPS_PROXY, like the TS); else `dial_direct(host, port)`. On dial error → reply `REP_HOST_UNREACHABLE` + close.
  5. **Grant**: reply `05 00 00 01 <BND.ADDR 0.0.0.0> <BND.PORT 0>` (a zeroed bind addr is standard/accepted), then `tokio::io::copy_bidirectional(client, upstream)`.
- [ ] **Integration tests** (real loopback, hand-rolled SOCKS client bytes): (a) allowed host (loopback echo upstream, allowlisted by IP) → REQUEST_GRANTED + the tunnel echoes; (b) denied host → `REP_NOT_ALLOWED`; (c) malformed DOMAINNAME with a CRLF/null byte → `is_valid_host` rejects → `REP_NOT_ALLOWED` (never dialed); (d) DOMAINNAME parse for a normal host. Commit (`feat(sandbox-runtime): hand-rolled SOCKS5 allowlist proxy + opaque tunnel (P5)`).

### Task 3: gates (+ optional Docker socks group)
- [ ] (Optional, if cheap) add a `socks` group to `scripts/verify-bwrap.sh` mirroring `netbridge` but with a stand-in SOCKS5 proxy (e.g. `ssh -D` or a tiny python socks) + `curl --socks5-hostname localhost:1080`: bwrap child reaches the allowed host through the :1080 bridge, direct blocked. If a stand-in SOCKS proxy is awkward in-container, SKIP this and note the tokio integration tests are the runtime proof (document).
- [ ] Final gates: `cargo test -p sandbox-runtime` + clippy `-D warnings` + `cargo test --workspace --no-run` + `cargo tree -p engine-mobile -e normal | grep -c sandbox-runtime` (0) + frozen diff empty. Stage ONLY explicit sandbox-runtime + scripts + Cargo paths.

## Final verification
1. SOCKS5 handshake + CONNECT (IPv4/DOMAINNAME/IPv6) ported; the pre-connect `is_valid_host` + filter gate runs BEFORE any dial (no bypass); parent-proxy routing (always-HTTPS_PROXY) + direct fallback; opaque tunnel.
2. Reply codes faithful (granted / not-allowed / host-unreachable / general-failure).
3. engine-mobile 0-dep; frozen empty.
