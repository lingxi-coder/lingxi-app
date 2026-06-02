//! Loopback HTTP listener for the OAuth redirect.
//!
//! See spec §30.3 / A5. Binds a TCP listener on `127.0.0.1:{port}`, accepts
//! connections until a `GET /callback?code=...&state=...` arrives (replying
//! `404` to any other path, e.g. a favicon probe), URL-decodes and validates
//! the `state` against the locally-generated CSRF token, writes a small HTML
//! success page to the browser, and returns the parsed params.
//!
//! Passing `port == 0` binds an OS-assigned ephemeral port; the caller reads
//! the chosen port back via [`CallbackListener::port`] and bakes it into the
//! `redirect_uri` before opening the browser (claude-code's `listen(0)`
//! pattern).

use std::net::SocketAddr;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Failures during the loopback callback handshake.
#[derive(Debug, Clone, Error)]
pub enum CallbackError {
    /// `bind` / `accept` failed at the socket layer.
    #[error("bind failed: {0}")]
    Bind(String),
    /// Request was malformed or missing required parameters.
    #[error("invalid callback request: {0}")]
    InvalidRequest(String),
    /// `state` returned by the `IdP` didn't match the local CSRF token.
    #[error("state mismatch")]
    StateMismatch,
}

/// Parameters parsed out of a `GET /callback?code=...&state=...` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallbackParams {
    /// Authorization code to exchange for tokens.
    pub code: String,
    /// CSRF state echoed back by the `IdP`.
    pub state: String,
}

/// A bound loopback listener whose port is known before any browser hits it.
///
/// Construct with [`CallbackListener::bind`] (which accepts `port == 0` for an
/// OS-assigned port), read [`CallbackListener::port`] into the `redirect_uri`,
/// then drive [`CallbackListener::accept`] to wait for the redirect.
pub struct CallbackListener {
    listener: TcpListener,
    port: u16,
}

impl CallbackListener {
    /// Bind `127.0.0.1:{port}`. `port == 0` selects an OS-assigned ephemeral
    /// port readable via [`Self::port`].
    ///
    /// # Errors
    /// [`CallbackError::Bind`] if the socket cannot be bound.
    pub async fn bind(port: u16) -> Result<Self, CallbackError> {
        let addr: SocketAddr = ([127, 0, 0, 1], port).into();
        let listener = TcpListener::bind(addr)
            .await
            .map_err(|e| CallbackError::Bind(e.to_string()))?;
        let port = listener
            .local_addr()
            .map_err(|e| CallbackError::Bind(e.to_string()))?
            .port();
        Ok(Self { listener, port })
    }

    /// The actual bound port (resolved even when `bind(0)` was used).
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Accept connections until a `GET /callback` request arrives, validate the
    /// `state`, reply with a success page, and return the params. Connections
    /// to any other path receive a `404` and are skipped.
    ///
    /// # Errors
    /// [`CallbackError::StateMismatch`] on a CSRF mismatch; other variants on a
    /// malformed request or socket failure.
    pub async fn accept(self, expected_state: &str) -> Result<CallbackParams, CallbackError> {
        loop {
            let (stream, _) = self
                .listener
                .accept()
                .await
                .map_err(|e| CallbackError::Bind(e.to_string()))?;
            match handle_connection(stream, expected_state).await {
                Ok(Some(params)) => return Ok(params),
                // Non-/callback path (e.g. favicon) — already 404'd; keep waiting.
                Ok(None) => continue,
                Err(e) => return Err(e),
            }
        }
    }
}

/// Read one request off `stream`. Returns `Ok(Some(params))` for a valid
/// `/callback`, `Ok(None)` for any other path (after replying 404), or an error.
async fn handle_connection(
    mut stream: TcpStream,
    expected_state: &str,
) -> Result<Option<CallbackParams>, CallbackError> {
    let mut buf = vec![0u8; 4096];
    let n = stream
        .read(&mut buf)
        .await
        .map_err(|e| CallbackError::InvalidRequest(e.to_string()))?;
    let req = String::from_utf8_lossy(&buf[..n]);

    let line = req
        .lines()
        .next()
        .ok_or_else(|| CallbackError::InvalidRequest("empty request".into()))?;
    let path = line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| CallbackError::InvalidRequest("no path".into()))?;
    let (route, query) = path.split_once('?').unwrap_or((path, ""));

    // Only the OAuth redirect path is interesting; everything else (favicon,
    // probes) gets a 404 and is skipped without aborting the flow.
    if route != "/callback" {
        let _ = write_response(&mut stream, "404 Not Found", "Not found.").await;
        return Ok(None);
    }

    let mut code = None;
    let mut state = None;
    for kv in query.split('&') {
        if let Some(v) = kv.strip_prefix("code=") {
            code = Some(url_decode(v));
        } else if let Some(v) = kv.strip_prefix("state=") {
            state = Some(url_decode(v));
        }
    }
    let code = code.ok_or_else(|| CallbackError::InvalidRequest("missing code".into()))?;
    let state = state.ok_or_else(|| CallbackError::InvalidRequest("missing state".into()))?;
    if state != expected_state {
        return Err(CallbackError::StateMismatch);
    }

    let _ = write_response(
        &mut stream,
        "200 OK",
        "Login complete. You can close this window.",
    )
    .await;

    Ok(Some(CallbackParams { code, state }))
}

/// Best-effort `percent`-decode. Base64url callback values contain no `%`, but
/// some providers escape them — round-trip them faithfully. On a malformed
/// escape we keep the raw substring rather than dropping data.
fn url_decode(raw: &str) -> String {
    match urlencoding::decode(raw) {
        Ok(decoded) => decoded.into_owned(),
        Err(_) => raw.to_string(),
    }
}

async fn write_response(
    stream: &mut TcpStream,
    status: &str,
    body: &str,
) -> Result<(), std::io::Error> {
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len(),
    );
    stream.write_all(resp.as_bytes()).await
}

/// Bind on `127.0.0.1:port`, wait for a single GET to `/callback?code=...&state=...`,
/// validate the state token, write a small success page, and return the params.
///
/// Thin wrapper over [`CallbackListener`] preserved for callers that pick the
/// port themselves (the port must already be bound into the `redirect_uri`).
///
/// # Errors
/// See [`CallbackError`].
pub async fn await_callback(
    port: u16,
    expected_state: &str,
) -> Result<CallbackParams, CallbackError> {
    let listener = CallbackListener::bind(port).await?;
    listener.accept(expected_state).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpStream;

    async fn send_get(port: u16, path_and_query: &str) -> String {
        let mut client = TcpStream::connect(("127.0.0.1", port)).await.expect("connect");
        let req = format!("GET {path_and_query} HTTP/1.1\r\nHost: localhost\r\n\r\n");
        client.write_all(req.as_bytes()).await.expect("write");
        let mut resp = Vec::new();
        // The server closes the connection after writing; read to EOF.
        let _ = client.read_to_end(&mut resp).await;
        String::from_utf8_lossy(&resp).into_owned()
    }

    #[tokio::test]
    async fn returns_params_on_valid_callback() {
        let listener = CallbackListener::bind(0).await.expect("bind");
        let port = listener.port();
        assert_ne!(port, 0, "ephemeral port resolved");

        let server = tokio::spawn(async move { listener.accept("S").await });
        let resp = send_get(port, "/callback?code=abc&state=S").await;

        let params = server.await.expect("join").expect("accept ok");
        assert_eq!(
            params,
            CallbackParams {
                code: "abc".into(),
                state: "S".into()
            }
        );
        assert!(resp.contains("200 OK"));
        assert!(resp.contains("Login complete"));
    }

    #[tokio::test]
    async fn state_mismatch_is_rejected() {
        let listener = CallbackListener::bind(0).await.expect("bind");
        let port = listener.port();
        let server = tokio::spawn(async move { listener.accept("EXPECTED").await });
        let _ = send_get(port, "/callback?code=abc&state=WRONG").await;
        let result = server.await.expect("join");
        assert!(matches!(result, Err(CallbackError::StateMismatch)));
    }

    #[tokio::test]
    async fn url_encoded_values_are_decoded() {
        let listener = CallbackListener::bind(0).await.expect("bind");
        let port = listener.port();
        // code contains a percent-escaped slash; state is plain.
        let server = tokio::spawn(async move { listener.accept("ST").await });
        let _ = send_get(port, "/callback?code=a%2Fb&state=ST").await;
        let params = server.await.expect("join").expect("ok");
        assert_eq!(params.code, "a/b");
    }

    #[tokio::test]
    async fn non_callback_path_is_404_then_callback_succeeds() {
        let listener = CallbackListener::bind(0).await.expect("bind");
        let port = listener.port();
        let server = tokio::spawn(async move { listener.accept("S").await });

        // A favicon probe should be 404'd and skipped.
        let favicon_resp = send_get(port, "/favicon.ico").await;
        assert!(favicon_resp.contains("404"), "favicon resp: {favicon_resp}");

        // The real callback then arrives and resolves the accept().
        let _ = send_get(port, "/callback?code=zzz&state=S").await;
        let params = server.await.expect("join").expect("ok");
        assert_eq!(params.code, "zzz");
    }

    #[tokio::test]
    async fn await_callback_wrapper_binds_explicit_port() {
        // Bind 0 first to discover a free port, drop it, then reuse for the
        // explicit-port wrapper. Small race window is acceptable in a unit test.
        let probe = CallbackListener::bind(0).await.expect("probe bind");
        let port = probe.port();
        drop(probe);

        let server = tokio::spawn(async move { await_callback(port, "S").await });
        // Give the wrapper a moment to bind.
        tokio::task::yield_now().await;
        let _ = send_get(port, "/callback?code=c&state=S").await;
        let params = server.await.expect("join").expect("ok");
        assert_eq!(params.code, "c");
    }
}
