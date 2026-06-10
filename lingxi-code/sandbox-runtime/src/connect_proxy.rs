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

/// Max bytes accepted for the request head before giving up (slowloris /
/// oversized-head guard).
const MAX_HEAD_BYTES: usize = 8192;

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
    // `early` is any payload the client sent AFTER the `\r\n\r\n` head terminator
    // (the TLS ClientHello often arrives in the same TCP segment). It MUST be
    // forwarded to the upstream before the opaque tunnel, or the handshake stalls.
    let (head, early) = read_request_head(&mut client).await?;
    // First line: `CONNECT host:port HTTP/1.1`
    let Some(target) = head
        .lines()
        .next()
        .and_then(|l| l.strip_prefix("CONNECT "))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(parse_connect_target)
    else {
        client.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n").await?;
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

    let Ok(mut upstream) = dial_direct(&host, port).await else {
        client.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n").await?;
        return Ok(());
    };
    client
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await?;

    // Forward any early client payload (e.g. the TLS ClientHello) that arrived
    // in the same segment as the CONNECT head, BEFORE the opaque tunnel begins.
    if !early.is_empty() {
        upstream.write_all(&early).await?;
    }

    // Opaque bidirectional tunnel (`upstream.pipe(socket); socket.pipe(upstream)`).
    let mut client = client;
    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
    Ok(())
}

/// Read bytes until the `\r\n\r\n` request-head terminator (bounded to
/// [`MAX_HEAD_BYTES`] to reject a slowloris / oversized head). Returns the head
/// as a lossy-UTF-8 String plus any raw bytes the client sent AFTER the
/// terminator (its first tunnel payload), which the caller must not drop.
async fn read_request_head(client: &mut TcpStream) -> std::io::Result<(String, Vec<u8>)> {
    let mut buf = Vec::with_capacity(256);
    let mut chunk = [0u8; 256];
    let mut term_end: Option<usize> = None;
    loop {
        let n = client.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            term_end = Some(pos + 4);
            break;
        }
        if buf.len() > MAX_HEAD_BYTES {
            break;
        }
    }
    let split = term_end.unwrap_or(buf.len());
    let early = buf.split_off(split);
    Ok((String::from_utf8_lossy(&buf).into_owned(), early))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::NetworkConfig;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    // A trivial upstream that accepts a connection and echoes one read.
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

    /// Send a raw CONNECT and return the proxy's status line + the live socket.
    async fn connect_via_proxy(proxy_port: u16, target: &str) -> (String, TcpStream) {
        let mut s = TcpStream::connect(("127.0.0.1", proxy_port)).await.unwrap();
        s.write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut buf = vec![0u8; 128];
        let n = s.read(&mut buf).await.unwrap();
        let line = String::from_utf8_lossy(&buf[..n])
            .lines()
            .next()
            .unwrap_or("")
            .to_string();
        (line, s)
    }

    #[tokio::test]
    async fn allowed_host_tunnels_and_denied_gets_403() {
        let (uhost, uport) = echo_upstream().await;
        // allow the loopback upstream by exact host; deny everything else
        let cfg = NetworkConfig {
            allowed_domains: vec![uhost.clone()],
            denied_domains: vec![],
        };
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

    /// Correctness-critical: bytes the client sends in the SAME segment as the
    /// CONNECT head terminator (`...\r\n\r\nEARLY`) — e.g. the TLS ClientHello —
    /// must reach the upstream, not be dropped at the head/tunnel boundary.
    #[tokio::test]
    async fn early_payload_after_head_is_preserved() {
        let (uhost, uport) = echo_upstream().await;
        let cfg = NetworkConfig {
            allowed_domains: vec![uhost.clone()],
            denied_domains: vec![],
        };
        let pport = start_proxy(cfg).await;

        let mut s = TcpStream::connect(("127.0.0.1", pport)).await.unwrap();
        // CONNECT head AND the early payload in one write (same segment).
        s.write_all(
            format!("CONNECT {uhost}:{uport} HTTP/1.1\r\nHost: {uhost}\r\n\r\nEARLY").as_bytes(),
        )
        .await
        .unwrap();

        // Drain the `200 Connection Established` status line, then the echoed
        // early payload (the upstream echoes whatever it reads first).
        let mut buf = vec![0u8; 256];
        let mut got = Vec::new();
        loop {
            let n = s.read(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            got.extend_from_slice(&buf[..n]);
            if got.windows(5).any(|w| w == b"EARLY") {
                break;
            }
        }
        let text = String::from_utf8_lossy(&got);
        assert!(text.contains("200"), "expected 200 status, got {text:?}");
        assert!(
            text.contains("EARLY"),
            "early payload lost; upstream echo was {text:?}"
        );
    }
}
