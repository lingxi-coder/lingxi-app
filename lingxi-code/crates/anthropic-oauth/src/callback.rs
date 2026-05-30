//! Loopback HTTP listener for the OAuth redirect.
//!
//! See spec §30.3 / A5. Binds a single TCP connection on `127.0.0.1:{port}`,
//! parses the GET request line for `code` and `state`, validates `state`
//! against the locally-generated CSRF token, and returns a small HTML page
//! to the browser.

use std::net::SocketAddr;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

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
pub struct CallbackParams {
    /// Authorization code to exchange for tokens.
    pub code: String,
    /// CSRF state echoed back by the `IdP`.
    pub state: String,
}

/// Bind on `127.0.0.1:port`, wait for a single GET to `/callback?code=...&state=...`,
/// validate the state token, write a small success page, and return the params.
pub async fn await_callback(
    port: u16,
    expected_state: &str,
) -> Result<CallbackParams, CallbackError> {
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|e| CallbackError::Bind(e.to_string()))?;
    let (mut stream, _) = listener
        .accept()
        .await
        .map_err(|e| CallbackError::Bind(e.to_string()))?;
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
    let query = path.split_once('?').map_or("", |(_, q)| q);
    let mut code = None;
    let mut state = None;
    for kv in query.split('&') {
        if let Some(v) = kv.strip_prefix("code=") {
            code = Some(v.to_string());
        }
        if let Some(v) = kv.strip_prefix("state=") {
            state = Some(v.to_string());
        }
    }
    let code = code.ok_or_else(|| CallbackError::InvalidRequest("missing code".into()))?;
    let state = state.ok_or_else(|| CallbackError::InvalidRequest("missing state".into()))?;
    if state != expected_state {
        return Err(CallbackError::StateMismatch);
    }

    let body = "Login complete. You can close this window.";
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(resp.as_bytes()).await;

    Ok(CallbackParams { code, state })
}
