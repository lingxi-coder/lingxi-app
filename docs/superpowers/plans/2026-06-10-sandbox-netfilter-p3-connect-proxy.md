# Sandbox Net-Filter P3 — Base CONNECT Allowlist Proxy (first runnable)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Port the base (non-MITM) HTTPS `CONNECT` forward-proxy from `http-proxy.js` — parse the CONNECT target, run the P1 allowlist filter, 403 on deny, else dial the origin directly + opaque bidirectional tunnel. Hand-rolled over tokio (no hyper/rustls). The first RUNNABLE artifact; Docker-verified (allowed host reachable, denied host 403).

**Architecture:** New module `lingxi-code/sandbox-runtime/src/connect_proxy.rs` + `dial.rs`. Adds `tokio` to the crate. The filter is the P1 `filter_network_request(port, host, &NetworkConfig)`.

**Scope (this batch = the CONNECT/HTTPS security core + dominant case):** CONNECT parse + allowlist 403 + direct-dial opaque tunnel + the accept loop. DEFERRED to later sub-steps (documented, NOT in P3): plain-HTTP (`http://`) full-URI forwarding (needs an HTTP/1.1 parser/hyper), parent-proxy CONNECT chaining (P2 has the resolver; `connect_via_parent_proxy` dialer is a follow-up), the `filterRequest` body hook (P6/MITM territory), MITM TLS termination (P6).

**Reference of truth:** `docs/superpowers/references/sandbox-runtime-0.0.54/dist/sandbox/http-proxy.js` (the `server.on('connect')` handler :12-145 + `parseConnectTarget` :265-279) and `parent-proxy.js:415` (`dialDirect`). Umbrella: `docs/.../2026-06-10-sandbox-netfilter-umbrella-design.md`.

**Branch:** `parity-sandbox-netfilter-p2p3` (P2 already committed on it).

**Conventions:** Cargo root `lingxi-code/`; git from repo root, `lingxi-code/...` paths. `-D missing-docs` + clippy pedantic `-D warnings`. `#![forbid(unsafe_code)]` on the crate. Commit `git commit -F`, footer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. **Docker daemon must be running** for the Task 3 runtime gate.

---

### Task 1: CONNECT target parsing + direct dialer

**Files:** Modify `sandbox-netfilter/Cargo.toml` (+tokio); Create `sandbox-netfilter/src/dial.rs`; Modify `src/lib.rs`.

- [ ] **Step 1: tokio dep.** Add to `[dependencies]`: `tokio = { workspace = true, features = ["rt", "rt-multi-thread", "net", "io-util", "time", "macros"] }`. Add to `[dev-dependencies]`: `tokio = { workspace = true, features = ["rt-multi-thread", "macros", "net", "io-util", "time"] }`.

- [ ] **Step 2: Failing test** for `parse_connect_target` in `dial.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_connect_target_host_and_ipv6() {
        // http-proxy.js:265-279
        assert_eq!(parse_connect_target("example.com:443"), Some(("example.com".into(), 443)));
        assert_eq!(parse_connect_target("[::1]:8443"), Some(("::1".into(), 8443)));
        assert_eq!(parse_connect_target("host:1"), Some(("host".into(), 1)));
        // invalid
        assert_eq!(parse_connect_target("noport"), None);
        assert_eq!(parse_connect_target("host:0"), None);
        assert_eq!(parse_connect_target("host:65536"), None);
        assert_eq!(parse_connect_target("host:abc"), None);
    }
}
```

- [ ] **Step 3: Verify fail**, then **implement** `dial.rs`:

```rust
//! CONNECT-target parsing + bounded direct dial (`http-proxy.js:265-279`,
//! `parent-proxy.js:415-437`).

use std::time::Duration;

use tokio::net::TcpStream;

/// Default connect timeout (`parent-proxy.js:21` `CONNECT_TIMEOUT_MS`).
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Parse a CONNECT request-target `host:port` (or `[ipv6]:port`) into
/// `(hostname, port)`. Port must be 1..=65535. (`parseConnectTarget`.)
#[must_use]
pub fn parse_connect_target(target: &str) -> Option<(String, u16)> {
    let (host, port_str) = if let Some(rest) = target.strip_prefix('[') {
        let (h, after) = rest.split_once(']')?;
        (h.to_string(), after.strip_prefix(':')?)
    } else {
        let (h, p) = target.rsplit_once(':')?;
        // reject if host part itself contains a colon (would be an unbracketed v6)
        if h.contains(':') {
            return None;
        }
        (h.to_string(), p)
    };
    let port: u32 = port_str.parse().ok()?;
    if !(1..=65535).contains(&port) {
        return None;
    }
    Some((host, port as u16))
}

/// Dial `host:port` directly with a bounded timeout (`dialDirect`).
///
/// # Errors
/// Returns the connect error or a timeout.
pub async fn dial_direct(host: &str, port: u16) -> std::io::Result<TcpStream> {
    match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((host, port))).await {
        Ok(r) => r,
        Err(_) => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "connect timed out",
        )),
    }
}
```

- [ ] **Step 4: lib.rs** `pub mod dial;`. **Run `cargo test -p sandbox-runtime` → PASS. Gate + commit** (`feat(sandbox-netfilter): CONNECT-target parse + direct dialer (P3)`).

---

### Task 2: the CONNECT proxy server

**Files:** Create `sandbox-netfilter/src/connect_proxy.rs`; Modify `src/lib.rs`.

- [ ] **Step 1: Failing integration test** (bottom of `connect_proxy.rs`) — drives a real proxy over loopback with an upstream echo server:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::NetworkConfig;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    // A trivial upstream that accepts a connection and echoes one line.
    async fn echo_upstream() -> (String, u16) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut s, _)) = l.accept().await {
                let mut buf = [0u8; 16];
                let n = s.read(&mut buf).await.unwrap_or(0);
                let _ = s.write_all(&buf[..n]).await;
            }
        });
        (addr.ip().to_string(), addr.port())
    }

    async fn start_proxy(cfg: NetworkConfig) -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let cfg = Arc::new(cfg);
        tokio::spawn(async move { serve_connect(l, cfg).await });
        port
    }

    /// Send a raw CONNECT and return the proxy's status line.
    async fn connect_via_proxy(proxy_port: u16, target: &str) -> (String, TcpStream) {
        let mut s = TcpStream::connect(("127.0.0.1", proxy_port)).await.unwrap();
        s.write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut buf = vec![0u8; 128];
        let n = s.read(&mut buf).await.unwrap();
        let line = String::from_utf8_lossy(&buf[..n]).lines().next().unwrap_or("").to_string();
        (line, s)
    }

    #[tokio::test]
    async fn allowed_host_tunnels_and_denied_gets_403() {
        let (uhost, uport) = echo_upstream().await;
        // allow the loopback upstream by exact host; deny everything else
        let cfg = NetworkConfig { allowed_domains: vec![uhost.clone()], denied_domains: vec![] };
        let pport = start_proxy(cfg).await;

        // allowed: 200 + the tunnel echoes
        let (line, mut tun) = connect_via_proxy(pport, &format!("{uhost}:{uport}")).await;
        assert!(line.contains("200"), "expected 200, got {line}");
        tun.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        tun.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ping");

        // denied: 403
        let (line, _) = connect_via_proxy(pport, "1.2.3.4:443").await;
        assert!(line.contains("403"), "expected 403, got {line}");
    }

    #[tokio::test]
    async fn malformed_connect_gets_400() {
        let pport = start_proxy(NetworkConfig::default()).await;
        let (line, _) = connect_via_proxy(pport, "noport").await;
        assert!(line.contains("400"), "expected 400, got {line}");
    }
}
```

NOTE: the test allowlists the loopback IP as an exact host. `filter_network_request` canonicalizes `127.0.0.1` and matches it exactly — confirm that works (a non-wildcard exact pattern of an IP literal). If the matcher's wildcard-vs-IP guard interferes, use a wildcard-free exact IP pattern (it should match via the exact branch). Report.

- [ ] **Step 2: Verify fail**, then **implement** `connect_proxy.rs`:

```rust
//! Base (non-MITM) HTTPS `CONNECT` forward proxy (`http-proxy.js` connect
//! handler). Parses CONNECT, runs the allowlist filter, 403 on deny, else
//! dials the origin directly and opaque-tunnels (no TLS interception — exactly
//! the base-sandbox behaviour, hostname-allowlisted). Hand-rolled over tokio.
//!
//! DEFERRED (later sub-projects): plain-HTTP forwarding, parent-proxy CONNECT
//! chaining, the `filterRequest` body hook, MITM termination.

use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::config::NetworkConfig;
use crate::dial::{dial_direct, parse_connect_target};
use crate::matcher::filter_network_request;

/// Run the CONNECT proxy accept loop on `listener`, filtering against `config`.
/// Each connection is handled on its own task; errors are logged-and-dropped.
pub async fn serve_connect(listener: TcpListener, config: Arc<NetworkConfig>) {
    loop {
        let Ok((client, _peer)) = listener.accept().await else {
            continue;
        };
        let config = Arc::clone(&config);
        tokio::spawn(async move {
            if let Err(e) = handle_connect(client, &config).await {
                tracing::debug!(error = %e, "connect proxy connection ended");
            }
        });
    }
}

/// Read the CONNECT request, filter, and either tunnel or reject.
async fn handle_connect(mut client: TcpStream, config: &NetworkConfig) -> std::io::Result<()> {
    let req = read_request_head(&mut client).await?;
    // First line: `CONNECT host:port HTTP/1.1`
    let Some(target) = req
        .lines()
        .next()
        .and_then(|l| l.strip_prefix("CONNECT "))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(parse_connect_target)
    else {
        client
            .write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n")
            .await?;
        return Ok(());
    };
    let (host, port) = target;

    if !filter_network_request(port, &host, config) {
        client
            .write_all(
                b"HTTP/1.1 403 Forbidden\r\n\
                  Content-Type: text/plain\r\n\
                  X-Proxy-Error: blocked-by-allowlist\r\n\
                  \r\n\
                  Connection blocked by network allowlist",
            )
            .await?;
        return Ok(());
    }

    let upstream = match dial_direct(&host, port).await {
        Ok(s) => s,
        Err(_) => {
            client.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n").await?;
            return Ok(());
        }
    };
    client
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await?;

    // Opaque bidirectional tunnel (`upstream.pipe(socket); socket.pipe(upstream)`).
    let mut client = client;
    let mut upstream = upstream;
    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
    Ok(())
}

/// Read bytes until the `\r\n\r\n` request-head terminator (bounded to 8 KiB to
/// reject a slowloris / oversized head). Returns the head as a String.
async fn read_request_head(client: &mut TcpStream) -> std::io::Result<String> {
    let mut buf = Vec::with_capacity(256);
    let mut chunk = [0u8; 256];
    loop {
        let n = client.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buf.len() > 8192 {
            break;
        }
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}
```

NOTE: add `tracing = { workspace = true }` to `[dependencies]` if not present (used for the debug log). If the workspace forbids `tracing` in this leaf crate, drop the log + the line. The `copy_bidirectional` head-forwarding caveat: a real client may send the TLS ClientHello bytes in the same segment as the CONNECT terminator — `read_request_head` stops at `\r\n\r\n`, so any bytes AFTER it in `buf` are the client's first payload and would be LOST. For correctness, capture the leftover (bytes after the `\r\n\r\n`) and `upstream.write_all(leftover)` before `copy_bidirectional`. IMPLEMENT THAT: split `buf` at the terminator, keep the tail, write it to `upstream` after the 200. Add a test asserting early-payload bytes survive (send `CONNECT ...\r\n\r\nEARLY` and assert the upstream echo includes `EARLY`).

- [ ] **Step 3: lib.rs** `pub mod connect_proxy;`. **Run `cargo test -p sandbox-runtime` → PASS (incl. the early-payload test). Gate + commit** (`feat(sandbox-netfilter): base CONNECT allowlist proxy with opaque tunnel (P3)`).

---

### Task 3: Docker runtime gate + final gates

- [ ] **Step 1: Docker proxy assertion.** Add a `connect-proxy` group to `scripts/verify-bwrap.sh` (or a new `scripts/verify-netproxy.sh`) that, INSIDE a `--privileged arm64v8/debian:stable-slim` container with bubblewrap+curl, runs the COMPILED proxy binary... — but the Rust proxy isn't trivially runnable in the container. SIMPLER faithful gate: the proxy logic is exercised by the Rust integration tests (Task 2) which ARE the runtime proof (real tokio sockets, real tunnel, real 403). For the Docker layer, assert the PROXY-SHAPE behavior with a stand-in: run a one-liner that proves `curl -x http://127.0.0.1:PORT https://host` uses CONNECT (so our CONNECT parser handles real curl framing). Use a tiny socat/ncat CONNECT echo OR — cleanest — SKIP a separate Docker gate for P3 (the tokio integration tests cover the real socket behavior) and defer the end-to-end Docker proof to P4, where the proxy runs on the host and a bwrap child curls through it. DOCUMENT this: P3's runtime proof = the tokio integration tests; the bwrap-child end-to-end proof lands in P4.

  Concretely for P3: run `cargo test -p sandbox-runtime -- --include-ignored` is not needed; just ensure the Task-2 integration tests pass (they bind real loopback sockets). Note in the commit that the full bwrap-child Docker gate is P4.

- [ ] **Step 2: Full gate ritual.**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-code
cargo test -p sandbox-runtime
cargo clippy -p sandbox-runtime --all-targets --no-deps -- -D warnings
cargo test --workspace --no-run
cargo build -p engine-mobile
cargo tree -p engine-mobile -e normal | grep -c "sandbox-runtime"  # 0
```

- [ ] **Step 3: Frozen check** `git diff main -- lingxi-code/traits lingxi-code/protocol` → empty.

- [ ] **Step 4: Commit** any fixups.

## Final verification
1. `cargo test -p sandbox-runtime` green (P1 8 + P2 4 + P3 dial/proxy tests incl. allow/deny/400/early-payload).
2. engine-mobile pulls 0 sandbox-netfilter (tokio is dev+lib dep of THIS crate only — confirm mobile graph clean).
3. Frozen empty.
4. The CONNECT path is the security core: allow→tunnel, deny→403, malformed→400, early-payload preserved. Plain-HTTP/parent-chaining/MITM deferred (documented).
