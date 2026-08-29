//! §19 — a static or headers-helper-minted `Authorization` header is
//! authoritative over OAuth for SSE/HTTP MCP servers.
//!
//! Oracle `bo(...)`/`Xt(...)` (services/mcp/auth.ts, byte offset ~@182269537
//! in the 2.1.251 binary) derives two independent booleans BEFORE any OAuth
//! provider is constructed:
//!   - `hasUserAuthHeader` — the server's static config already carries an
//!     `Authorization` header;
//!   - `helperMintsAuthHeader` — the configured `headersHelper`'s dynamic
//!     output actually contains an `Authorization` key (not merely that a
//!     helper is configured — a helper may mint some other header entirely).
//!
//! Either one is authoritative: OAuth is never constructed (so its Bearer
//! can never overwrite the value), checked in that order (`hasUserAuthHeader`
//! first) matching the oracle's `if(h){...} if(_){...}` chain. If the
//! resulting connect attempt still fails with an auth-type response (401 or
//! 403), the failure is reported using the oracle's exact copy —
//! `AUTH_HEADER_REJECTED` / `HEADERS_HELPER_AUTH_REJECTED` — instead of the
//! raw transport error.
//!
//! `FIRST_PARTY_AUTH_REJECTED` (the third code in the same oracle `if` chain,
//! gated on `useFirstPartyAuth`/`firstPartyBearer` with values
//! `design_credential` | `none` | `login` | `design_scoped_login`) is
//! deliberately NOT modelled here: every branch of its message text is about
//! a claude.ai/`/login`/Claude Design first-party bearer — the
//! `claudeai-proxy` connector surface, an explicit non-goal for this port
//! (no first-party-auth concept exists in the Rust registry to trigger it).

use crate::registry::error_is_auth_response;
use traits::{McpError, McpTransportSpec};

/// `hasUserAuthHeader`: does the server's STATIC config — before any
/// `headersHelper` run or OAuth injection — already carry an `Authorization`
/// header? Only SSE/HTTP/WebSocket specs carry headers at all.
pub(crate) fn spec_has_authorization(spec: &McpTransportSpec) -> bool {
    match spec {
        McpTransportSpec::Sse { headers, .. }
        | McpTransportSpec::Http { headers, .. }
        | McpTransportSpec::WebSocket { headers, .. } => headers.contains_key("Authorization"),
        _ => false,
    }
}

/// Best-known HTTP status for an auth-type connect failure, mirroring the
/// oracle's `${statusCode ?? 401}` default. `error_is_auth_response` (this
/// function's only caller-side gate) classifies exclusively on a 401/403, so
/// the `_ => 401` arm below is unreachable in practice — kept only for
/// defensive parity with the oracle's fallback.
fn auth_failure_status(e: &McpError) -> u16 {
    match e {
        McpError::HttpResponse { status, .. } => *status,
        McpError::Connection(m) | McpError::Handshake(m) if m.contains("403") => 403,
        _ => 401,
    }
}

/// `AUTH_HEADER_REJECTED` — byte-exact message from the 2.1.251 binary.
fn auth_header_rejected(e: &McpError) -> McpError {
    let status = auth_failure_status(e);
    McpError::Connection(format!(
        "Server rejected the configured Authorization header (HTTP {status}). Check that the token is valid for this MCP endpoint — OAuth fallback is disabled when headers.Authorization is set."
    ))
}

/// `HEADERS_HELPER_AUTH_REJECTED` — byte-exact message from the 2.1.251
/// binary.
fn headers_helper_auth_rejected(e: &McpError) -> McpError {
    let status = auth_failure_status(e);
    McpError::Connection(format!(
        "Server rejected the Authorization header minted by the configured headersHelper (HTTP {status}). Check that the helper command returns a valid credential for this MCP endpoint — OAuth fallback is disabled when the helper supplies Authorization."
    ))
}

/// Reclassify a connect failure per the oracle's `hasUserAuthHeader` /
/// `helperMintsAuthHeader` precedence (static checked before helper-minted).
/// A non-auth-type failure, or one where neither flag is set, passes through
/// unchanged — safe to call at every connect-failure exit point.
pub(crate) fn classify_auth_failure(
    error: McpError,
    has_user_auth_header: bool,
    helper_minted_authorization: bool,
) -> McpError {
    if !error_is_auth_response(&error) {
        return error;
    }
    if has_user_auth_header {
        return auth_header_rejected(&error);
    }
    if helper_minted_authorization {
        return headers_helper_auth_rejected(&error);
    }
    error
}

#[cfg(test)]
mod tests {
    use super::*;

    fn http_spec(headers: &[(&str, &str)]) -> McpTransportSpec {
        McpTransportSpec::Http {
            url: "https://mcp.example".into(),
            headers: traits::McpHeaders::from_iter(
                headers
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string())),
            ),
            headers_helper: None,
            oauth: None,
        }
    }

    #[test]
    fn spec_has_authorization_detects_static_header() {
        assert!(spec_has_authorization(&http_spec(&[(
            "Authorization",
            "Bearer x"
        )])));
        assert!(!spec_has_authorization(&http_spec(&[("X-Static", "yes")])));
    }

    #[test]
    fn classify_passes_through_non_auth_failures() {
        let e = McpError::Connection("connection refused".into());
        let out = classify_auth_failure(e, true, false);
        assert!(matches!(out, McpError::Connection(m) if m == "connection refused"));
    }

    #[test]
    fn classify_passes_through_when_neither_flag_set() {
        let e = McpError::HttpResponse {
            status: 401,
            www_authenticate: None,
        };
        let out = classify_auth_failure(e, false, false);
        assert!(matches!(out, McpError::HttpResponse { status: 401, .. }));
    }

    #[test]
    fn classify_prefers_static_header_over_helper_minted() {
        // Both flags true (an unusual but possible config): the oracle's
        // `if(h){...} if(_){...}` chain checks `hasUserAuthHeader` first.
        let e = McpError::HttpResponse {
            status: 401,
            www_authenticate: None,
        };
        let out = classify_auth_failure(e, true, true);
        match out {
            McpError::Connection(m) => assert!(m.starts_with(
                "Server rejected the configured Authorization header (HTTP 401)."
            )),
            other => panic!("expected AUTH_HEADER_REJECTED copy, got {other:?}"),
        }
    }

    #[test]
    fn auth_header_rejected_copy_is_byte_exact() {
        let e = McpError::HttpResponse {
            status: 403,
            www_authenticate: None,
        };
        let out = classify_auth_failure(e, true, false);
        assert_eq!(
            out.to_string(),
            "connection failed: Server rejected the configured Authorization header (HTTP 403). Check that the token is valid for this MCP endpoint — OAuth fallback is disabled when headers.Authorization is set."
        );
    }

    #[test]
    fn headers_helper_auth_rejected_copy_is_byte_exact() {
        let e = McpError::Connection("HTTP 401 Unauthorized".into());
        let out = classify_auth_failure(e, false, true);
        assert_eq!(
            out.to_string(),
            "connection failed: Server rejected the Authorization header minted by the configured headersHelper (HTTP 401). Check that the helper command returns a valid credential for this MCP endpoint — OAuth fallback is disabled when the helper supplies Authorization."
        );
    }
}
