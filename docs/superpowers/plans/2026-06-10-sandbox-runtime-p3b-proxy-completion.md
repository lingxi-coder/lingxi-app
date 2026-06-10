# sandbox-runtime P3b — Proxy Completion (parent-proxy dialers + hyper http-proxy + request-filter)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Finish `parent-proxy.js` and `http-proxy.js` (+ `request-filter.js`) to full 1:1 behavior. Adds: `open_connect_tunnel`/`connect_via_parent_proxy`/`proxy_auth_header`/`strip_hop_by_hop`/`redact_url`; and a hyper-1.x forward proxy handling BOTH plain-HTTP (full-URI forwarding) and `CONNECT` (upgrade → tunnel), with parent-proxy routing + the `filterRequest` body hook. Retires the P3 hand-rolled `connect_proxy::serve_connect` in favor of the unified hyper server (keeping `dial::{parse_connect_target, dial_direct}`).

**Architecture:** Node uses a single `http.Server` for both `connect` and `request` events; the faithful Rust port is one `hyper` 1.x server with a CONNECT-upgrade handler + a plain-HTTP service. New modules in `lingxi-code/sandbox-runtime/src/`: extend `parent_proxy.rs`, add `request_filter.rs`, add `http_proxy.rs` (the hyper server). `connect_proxy.rs`'s `serve_connect` is removed; its `handle_connect` tunnel logic moves into the hyper CONNECT-upgrade handler.

**Reference of truth:** `docs/superpowers/references/sandbox-runtime-0.0.54/dist/sandbox/{parent-proxy.js (openConnectTunnel :214-283, connectViaParentProxy :289-313, proxyAuthHeader :316-330, stripHopByHop :326-342 [HOP_BY_HOP set :28-38], redactUrl :347-355), http-proxy.js (connect handler :12-145, request handler :147-258, parseConnectTarget :265-279), request-filter.js (decideAndRespond, BODYLESS_METHODS, incomingHeaders, deny)}`.

**Deps (all in lock except where noted):** `hyper = { version = "1", features = ["server","client","http1"] }`, `hyper-util = { version = "0.1", features = ["tokio","server","client-legacy"] }`, `http-body-util = "0.1"`, `tokio-rustls = "0.25"` + `rustls = "0.22"` (for the https-parent-proxy CONNECT dial), `rustls-native-certs` or `webpki-roots` for the parent-TLS roots. NO `rcgen` here (that's P6 MITM).

**Branch:** `parity-sandbox-runtime-p3b`. **Conventions:** Cargo root `lingxi-code/`; git from repo root with `lingxi-code/...` paths — **`git add` ONLY explicit sandbox-runtime/doc paths, NEVER `git add -A`** (untracked `codex/`/`liter-llm/`/`opencode/` dirs exist at repo root). `-D missing-docs` + clippy pedantic `-D warnings`. `#![forbid(unsafe_code)]`. Commit `git commit -F`, footer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. Docker running (P4 lands the bwrap e2e; P3b's runtime proof is hyper integration tests over loopback).

---

### Task 1: parent-proxy completion (dialers + utils)

**Files:** Modify `sandbox-runtime/src/parent_proxy.rs`; Modify `Cargo.toml` (+tokio-rustls/rustls + roots).

- [ ] **Step 1: Failing tests** (append to `parent_proxy.rs` tests):

```rust
    #[test]
    fn strip_hop_by_hop_removes_standard_and_connection_listed() {
        // parent-proxy.js:28-38 + 326-342
        let mut h = vec![
            ("Host".to_string(), "x".to_string()),
            ("Connection".to_string(), "keep-alive, X-Custom".to_string()),
            ("Keep-Alive".to_string(), "timeout=5".to_string()),
            ("Transfer-Encoding".to_string(), "chunked".to_string()),
            ("X-Custom".to_string(), "drop-me".to_string()),
            ("Accept".to_string(), "*/*".to_string()),
        ];
        let out = strip_hop_by_hop(&h);
        let names: Vec<String> = out.iter().map(|(k, _)| k.to_ascii_lowercase()).collect();
        assert!(names.contains(&"host".to_string()));
        assert!(names.contains(&"accept".to_string()));
        assert!(!names.contains(&"connection".to_string()));
        assert!(!names.contains(&"keep-alive".to_string()));
        assert!(!names.contains(&"transfer-encoding".to_string()));
        assert!(!names.contains(&"x-custom".to_string())); // named in Connection
        let _ = &mut h;
    }

    #[test]
    fn proxy_auth_header_basic_and_none() {
        assert_eq!(proxy_auth_header(&url::Url::parse("http://u:p@x:3128").unwrap()).as_deref(),
                   Some("Basic dTpw")); // base64("u:p")
        assert!(proxy_auth_header(&url::Url::parse("http://x:3128").unwrap()).is_none());
    }

    #[test]
    fn redact_url_hides_userinfo() {
        assert_eq!(redact_url(Some(&url::Url::parse("http://u:p@x:3128/").unwrap())),
                   "http://***:***@x:3128/");
        assert_eq!(redact_url(Some(&url::Url::parse("http://x:3128/").unwrap())), "http://x:3128/");
        assert_eq!(redact_url(None), "-");
    }

    #[tokio::test]
    async fn open_connect_tunnel_through_a_fake_proxy() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::{TcpListener, TcpStream};
        // fake parent proxy: accept, read the CONNECT, reply 200, then echo
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let pport = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut buf = [0u8; 256];
            let _ = s.read(&mut buf).await.unwrap();
            s.write_all(b"HTTP/1.1 200 OK\r\n\r\nLEFTOVER").await.unwrap();
            // echo whatever the client sends next
            let mut b2 = [0u8; 16];
            let n = s.read(&mut b2).await.unwrap_or(0);
            let _ = s.write_all(&b2[..n]).await;
        });
        let mut tun = open_connect_tunnel(
            || Box::pin(async move { TcpStream::connect(("127.0.0.1", pport)).await }),
            "example.com",
            443,
            None,
        )
        .await
        .expect("tunnel");
        // The post-200 LEFTOVER bytes must be preserved (unshift semantics).
        let mut lead = [0u8; 8];
        tun.read_exact(&mut lead).await.unwrap();
        assert_eq!(&lead, b"LEFTOVER");
    }

    #[tokio::test]
    async fn open_connect_tunnel_rejects_non_2xx_and_bad_host() {
        use tokio::net::{TcpListener, TcpStream};
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let pport = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let (mut s, _) = l.accept().await.unwrap();
            s.write_all(b"HTTP/1.1 403 Forbidden\r\n\r\n").await.unwrap();
        });
        assert!(open_connect_tunnel(|| Box::pin(async move { TcpStream::connect(("127.0.0.1", pport)).await }),
                                    "example.com", 443, None).await.is_err());
        // CRLF-injection host rejected before dialing
        assert!(open_connect_tunnel(|| Box::pin(async { TcpStream::connect(("127.0.0.1", 1)).await }),
                                    "evil\r\n.com", 443, None).await.is_err());
    }
```

- [ ] **Step 2: Verify fail**, then **implement** in `parent_proxy.rs`:

```rust
use std::future::Future;
use std::pin::Pin;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Hop-by-hop / proxy headers stripped before forwarding (`HOP_BY_HOP`,
/// parent-proxy.js:28-38).
const HOP_BY_HOP: [&str; 9] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// `stripHopByHop` (parent-proxy.js:326-342): drop hop-by-hop headers plus any
/// header named in the incoming `Connection` token list (RFC 7230 §6.1).
#[must_use]
pub fn strip_hop_by_hop(headers: &[(String, String)]) -> Vec<(String, String)> {
    let mut extra: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (k, v) in headers {
        if k.eq_ignore_ascii_case("connection") {
            for tok in v.split(',') {
                extra.insert(tok.trim().to_ascii_lowercase());
            }
        }
    }
    headers
        .iter()
        .filter(|(k, _)| {
            let lk = k.to_ascii_lowercase();
            !HOP_BY_HOP.contains(&lk.as_str()) && !extra.contains(&lk)
        })
        .cloned()
        .collect()
}

/// `proxyAuthHeader` (parent-proxy.js:316-330): `Basic base64(user:pass)` from
/// the proxy URL userinfo, percent-decoded; `None` if no credentials.
#[must_use]
pub fn proxy_auth_header(proxy_url: &url::Url) -> Option<String> {
    let user = proxy_url.username();
    let pass = proxy_url.password().unwrap_or("");
    if user.is_empty() && pass.is_empty() {
        return None;
    }
    // percent-decode (fall back to raw on malformed, like the TS try/catch).
    let dec = |s: &str| {
        percent_decode(s).unwrap_or_else(|| s.to_string())
    };
    let creds = format!("{}:{}", dec(user), dec(pass));
    Some(format!("Basic {}", base64_encode(creds.as_bytes())))
}

/// `redactUrl` (parent-proxy.js:347-355).
#[must_use]
pub fn redact_url(u: Option<&url::Url>) -> String {
    let Some(u) = u else { return "-".to_string() };
    if u.username().is_empty() && u.password().is_none_or(str::is_empty) {
        return u.as_str().to_string();
    }
    let mut c = u.clone();
    let _ = c.set_username("***");
    let _ = c.set_password(Some("***"));
    c.as_str().to_string()
}

/// `openConnectTunnel` (parent-proxy.js:214-283): given a `dial` future that
/// yields a connected stream, send `CONNECT host:port`, await a 2xx status,
/// and return the stream with any post-header bytes preserved at the front.
///
/// # Errors
/// Invalid host/port, dial error, non-2xx, oversized (>16KiB) header, or close
/// during handshake.
pub async fn open_connect_tunnel<F>(
    dial: impl FnOnce() -> F,
    dest_host: &str,
    dest_port: u16,
    auth_header: Option<&str>,
) -> std::io::Result<TunnelStream>
where
    F: Future<Output = std::io::Result<TcpStream>>,
{
    let bare = crate::host::strip_brackets(dest_host);
    if !crate::host::is_valid_host(&bare) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid destination host for CONNECT",
        ));
    }
    if dest_port == 0 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid port"));
    }
    let authority = if bare.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("[{bare}]:{dest_port}")
    } else {
        format!("{bare}:{dest_port}")
    };
    let mut sock = dial().await?;
    let req = format!(
        "CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n{}\r\n",
        auth_header.map(|a| format!("Proxy-Authorization: {a}\r\n")).unwrap_or_default()
    );
    sock.write_all(req.as_bytes()).await?;
    // Read the response head up to \r\n\r\n (cap 16 KiB).
    let mut buf: Vec<u8> = Vec::with_capacity(256);
    let mut chunk = [0u8; 256];
    let end = loop {
        let n = sock.read(&mut chunk).await?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "proxy closed during CONNECT handshake",
            ));
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos;
        }
        if buf.len() > 16 * 1024 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "CONNECT header too large"));
        }
    };
    let status_line = std::str::from_utf8(&buf[..buf.iter().position(|&b| b == b'\r').unwrap_or(buf.len())])
        .unwrap_or("");
    // `^HTTP/1.[01] 2\d\d`
    let ok = status_line.strip_prefix("HTTP/1.")
        .and_then(|r| r.strip_prefix('0').or_else(|| r.strip_prefix('1')))
        .and_then(|r| r.strip_prefix(' '))
        .is_some_and(|code| code.as_bytes().first() == Some(&b'2') && code.len() >= 3
            && code.as_bytes()[1].is_ascii_digit() && code.as_bytes()[2].is_ascii_digit());
    if !ok {
        return Err(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            format!("proxy refused CONNECT: {}", status_line.trim()),
        ));
    }
    let leftover = buf[end + 4..].to_vec();
    Ok(TunnelStream { inner: sock, leftover, leftover_pos: 0 })
}

/// A tunnelled stream that replays bytes received after the CONNECT response
/// header (the TS `sock.unshift(rest)`).
pub struct TunnelStream {
    inner: TcpStream,
    leftover: Vec<u8>,
    leftover_pos: usize,
}
// Implement tokio AsyncRead (drain `leftover` first, then `inner`) + AsyncWrite
// (delegate to `inner`). See the impl block in the implementer notes.

/// `connectViaParentProxy` (parent-proxy.js:289-313): dial the parent proxy
/// (TCP or TLS by scheme) and open a CONNECT tunnel through it.
///
/// # Errors
/// Dial/TLS/tunnel errors.
pub async fn connect_via_parent_proxy(
    proxy_url: &url::Url,
    dest_host: &str,
    dest_port: u16,
) -> std::io::Result<TunnelStream> {
    let proxy_host = crate::host::strip_brackets(proxy_url.host_str().unwrap_or(""));
    let proxy_port = proxy_url.port().unwrap_or(if proxy_url.scheme() == "https" { 443 } else { 80 });
    let auth = proxy_auth_header(proxy_url);
    if proxy_url.scheme() == "https" {
        // TLS dial via tokio-rustls (SNI = proxy_host unless it's an IP).
        // … implementer wires a rustls ClientConfig with system/webpki roots,
        // connects, then open_connect_tunnel over the TLS stream. (For the
        // TLS branch, TunnelStream must be generic over the stream type OR an
        // enum {Plain(TcpStream), Tls(TlsStream<TcpStream>)} — see notes.)
        return Err(std::io::Error::new(std::io::ErrorKind::Unsupported,
            "https parent proxy: implement TLS dial (notes)"));
    }
    open_connect_tunnel(
        || async move { TcpStream::connect((proxy_host.as_str(), proxy_port)).await },
        dest_host,
        dest_port,
        auth.as_deref(),
    )
    .await
}
```

IMPLEMENTER NOTES for Task 1:
- `TunnelStream` must impl `tokio::io::AsyncRead` (serve `leftover[leftover_pos..]` first, then poll `inner`) + `AsyncWrite`/`AsyncShutdown` (delegate to `inner`). For the https-parent path, make the inner transport an enum `Transport { Plain(TcpStream), Tls(tokio_rustls::client::TlsStream<TcpStream>) }` and impl AsyncRead/Write by matching — OR make `TunnelStream<S>` generic over `S: AsyncRead+AsyncWrite+Unpin` and return a boxed `Pin<Box<dyn AsyncRead+AsyncWrite+Unpin+Send>>` from both branches. Pick the cleaner one; the http_proxy (Task 2) just needs an `AsyncRead+AsyncWrite+Unpin+Send` tunnel. Implement the https-parent TLS dial fully (don't leave the `Unsupported` stub) — use `tokio-rustls` 0.25 + roots from `webpki-roots` (add the dep) or `rustls-native-certs`; SNI = `ServerName::try_from(proxy_host)` only when it's not an IP literal.
- `percent_decode` + `base64_encode`: use small dependencies already in the workspace if present (`base64`? check the lock — if `base64` is in the lock add it; else hand-roll the Basic base64, it's trivial) and `percent-encoding` (in lock via url? add `percent-encoding` if needed). Keep it faithful: percent-decode userinfo, base64 the `user:pass`.
- The `open_connect_tunnel` `dial` closure signature in the tests uses `|| Box::pin(async move {...})` returning a `TcpStream` future — align the real signature so the tests compile (a generic `FnOnce() -> F where F: Future<Output=io::Result<TcpStream>>` works for the TCP tests; the parent-TLS path constructs its own transport internally, so `connect_via_parent_proxy` doesn't go through the generic `dial` for TLS — it builds the TunnelStream over the TLS transport directly. Refactor `open_connect_tunnel` to be generic over the stream `S: AsyncRead+AsyncWrite+Unpin` and take an already-connected `S` (do the dial in the callers) — that's cleaner than the closure. Adjust the tests to pass a connected stream. DECIDE and keep the tests meaningful (leftover preserved, non-2xx rejected, bad-host rejected).

- [ ] **Step 3: Run `cargo test -p sandbox-runtime` → PASS. Gate + commit** (`feat(sandbox-runtime): parent-proxy CONNECT tunnel + dialers + hop-by-hop/auth/redact (P3b)`).

---

### Task 2: hyper forward proxy (plain-HTTP + CONNECT) + request-filter

**Files:** Create `sandbox-runtime/src/request_filter.rs`, `sandbox-runtime/src/http_proxy.rs`; Modify `src/lib.rs` (add modules; remove `connect_proxy`'s `serve_connect` — keep `dial`); Modify `Cargo.toml` (+hyper stack).

This task builds the unified proxy. Because the hyper 1.x forward-proxy + CONNECT-upgrade API is intricate, the plan specifies BEHAVIOR + the exact reference; the implementer writes idiomatic hyper 1.9 code.

- [ ] **Step 1: request_filter.rs** — port `decideAndRespond` (request-filter.js). Signature adapted to hyper: given the `filterRequest` callback (a `dyn Fn(Request parts) -> Future<Decision>`), the parsed request parts + body, the absolute URL, run the callback; on `deny` return a 403 (`X-Proxy-Error: blocked-by-sandbox-runtime`, body = reason + "\n"); on `allow` return the body to forward. Body-tee: for non-GET/HEAD/OPTIONS, the body must be readable by BOTH the callback and the upstream (tee). With hyper/http-body, collect-or-tee the body; faithful behavior = the callback sees the same bytes as upstream, and if the callback never reads, don't buffer the whole upload (best-effort; a bounded buffer is acceptable with a doc note if full tee-without-buffering is impractical). `BODYLESS_METHODS = {GET, HEAD, OPTIONS}`. Add unit tests: allow passes body through; deny returns 403 with the exact header + reason body; malformed → deny.

- [ ] **Step 2: http_proxy.rs** — the hyper server. ONE `ProxyOptions { config: Arc<NetworkConfig>, parent_proxy: Option<Arc<ResolvedParentProxy>>, filter_request: Option<Arc<dyn ...>> }` (no MITM fields yet — P6). Behavior:
  - **`serve(listener, options)`** accept loop → per connection, hyper `http1::Builder::serve_connection(..).with_upgrades()` over a service.
  - **plain HTTP request** (`http-proxy.js:147-258`): the request URI is absolute-form (`http://host/path`). Parse host+port (default 443 for https-scheme else 80). `allowed = filter_network_request(port, host, &config)` → if not, **403** with `X-Proxy-Error: blocked-by-allowlist` + body `Connection blocked by network allowlist` (byte-exact). Else: reconstruct the absolute URI from PARSED components (close URL-parser differential bypass), `strip_hop_by_hop(headers)` + set `Host`, run `request_filter::decide_and_respond` if `filter_request` set, then route: parent-proxy (when set and `!should_bypass_parent_proxy`) via a forward to the parent with `Proxy-Authorization` + absolute path, else **direct** to the origin. Use a hyper client (`hyper-util` legacy client or a hand-built `http1::handshake` to the origin). Relay the response with `strip_hop_by_hop` on the response headers. On upstream error → 502.
  - **CONNECT** (`http-proxy.js:12-145`): `filter(port, hostname)` → 403 (byte-exact, as P3) if denied; else dial — parent-proxy CONNECT (`connect_via_parent_proxy`) when set + not bypassed, else `dial_direct` — then `200 Connection Established`, `hyper::upgrade::on(req)` to get the client stream, write any early bytes, and `copy_bidirectional(upgraded, upstream)`. (Reuse the P3 tunnel semantics incl. early-payload.)
  - parseConnectTarget reuses `dial::parse_connect_target`.
  - **Retire** `connect_proxy::serve_connect` + `handle_connect` (moved here); keep `dial.rs`. Update the P3 integration tests to drive the new hyper server (allow→tunnel, deny→403, malformed→400, early-payload) — they must still pass.

- [ ] **Step 3: Integration tests** (real loopback): (a) CONNECT allow→tunnel + deny→403 + early-payload (ported from P3); (b) plain-HTTP allow→forwarded-to-origin (stand up a tiny hyper origin returning 200 "ok", proxy it, assert body); (c) plain-HTTP deny→403 byte-exact; (d) hop-by-hop headers stripped on forward (origin asserts it didn't receive `Connection`/a Connection-listed header). Note Docker e2e is P4.

- [ ] **Step 4: lib.rs** wire modules, drop `serve_connect`. **Run `cargo test -p sandbox-runtime` → PASS. Gate + commit** (`feat(sandbox-runtime): unified hyper forward proxy (plain-HTTP + CONNECT) + request-filter (P3b)`).

---

### Task 3: final gates

```bash
cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-code
cargo test -p sandbox-runtime
cargo clippy -p sandbox-runtime --all-targets --no-deps -- -D warnings
cargo test --workspace --no-run
cargo build -p engine-mobile
cargo tree -p engine-mobile -e normal | grep -c "sandbox-runtime"  # 0
```
Frozen check `git diff main -- lingxi-code/traits lingxi-code/protocol` → empty. **Stage ONLY `lingxi-code/sandbox-runtime` + `lingxi-code/Cargo.toml` + `lingxi-code/Cargo.lock` + the plan/doc — never `git add -A`.**

## Final verification
1. `cargo test -p sandbox-runtime` green (all prior + Task1 dialers + Task2 proxy/request-filter).
2. `http-proxy.js` + `parent-proxy.js` + `request-filter.js` fully ported (no remaining "deferred" remnants from those three files; MITM routing hooks left as `None`-typed seams for P6, documented).
3. engine-mobile 0-dep; frozen empty.
4. The CONNECT 403 + plain-HTTP 403 bodies are byte-exact; hop-by-hop stripped; parent-proxy chaining works (TCP + TLS parent); early-payload preserved.
