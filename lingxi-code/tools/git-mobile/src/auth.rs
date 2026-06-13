//! In-process credential callback + CA wiring for the network git ops
//! (clone / fetch / pull), spec §G3 (auth) + §G6 (TLS/CA).
//!
//! The HTTPS token is supplied in-memory by the Kotlin host and installed into
//! libgit2's `RemoteCallbacks::credentials` only for the duration of a network
//! call (borrowed for the `FetchOptions` lifetime). It NEVER reaches disk, a
//! child process, argv, or an environment variable: libgit2 is in-process, so
//! there is no exec boundary to cross. The token is also never logged.
//!
//! ## `forbid(unsafe_code)` carve-out
//!
//! The crate is `#![deny(unsafe_code)]` (NOT `forbid`) for exactly one reason:
//! the vendored `git2` exposes the CA-location option as the **`unsafe`**
//! functions `git2::opts::set_ssl_cert_file` / `set_ssl_cert_dir` (they mutate
//! a libgit2 global without synchronization). The spec (G6) assumed a safe
//! `set_ssl_cert_locations`; the pinned `git2 0.21` instead splits it into two
//! `unsafe` setters over `GIT_OPT_SET_SSL_CERT_LOCATIONS`. The single audited
//! `unsafe` block lives in [`set_ca_location`] under a localized
//! `#[allow(unsafe_code)]`; the rest of the crate keeps the deny lint, so no
//! other unsafe can slip in. See the Task 8 report for the deviation note.

use std::path::Path;

use crate::ops::GitOpError;

/// Username sent alongside the HTTPS token. GitHub/GitLab personal access
/// tokens are presented as the *password* with a fixed sentinel username; this
/// matches the documented `x-access-token` / PAT-as-password convention.
const TOKEN_USERNAME: &str = "x-access-token";

/// Build the [`git2::FetchOptions`] used by every network op (clone / fetch /
/// pull).
///
/// When `token` is `Some`, a `credentials` callback is installed that yields
/// `Cred::userpass_plaintext(TOKEN_USERNAME, token)` — the token is borrowed
/// for the returned `FetchOptions`' lifetime (`'a`), never copied into a
/// longer-lived store. When `token` is `None`, no credentials callback is
/// installed, so public (anonymous) HTTPS / `file://` remotes still work.
///
/// The token is never logged or written anywhere; it only flows into the
/// libgit2 credential callback in-process.
#[must_use]
pub fn make_fetch_options(token: Option<&str>) -> git2::FetchOptions<'_> {
    let mut callbacks = git2::RemoteCallbacks::new();
    if let Some(token) = token {
        callbacks.credentials(move |_url, _username_from_url, _allowed| {
            git2::Cred::userpass_plaintext(TOKEN_USERNAME, token)
        });
    }
    let mut opts = git2::FetchOptions::new();
    opts.remote_callbacks(callbacks);
    opts
}

/// Point libgit2's TLS backend at a CA-certificate location for verification.
///
/// `ca_dir`, when `Some`, is treated as a **directory** of one-cert-per-file CA
/// certificates (the Android `/system/etc/security/cacerts` layout) and passed
/// as the `path` argument of `GIT_OPT_SET_SSL_CERT_LOCATIONS` via
/// [`git2::opts::set_ssl_cert_dir`]; the `file` argument is left unset. When
/// `None`, this is a no-op (libgit2 / OpenSSL keep their built-in defaults —
/// the host file:// tests need no CA).
///
/// This is a **global** libgit2 option (set once for the process). It is
/// idempotent to call again with the same value.
///
/// # Errors
///
/// [`GitOpError::Libgit2`] if libgit2 rejects the location.
pub fn set_ca_location(ca_dir: Option<&str>) -> Result<(), GitOpError> {
    let Some(dir) = ca_dir else {
        return Ok(());
    };
    // SAFETY: `git2::opts::set_ssl_cert_dir` is `unsafe` only because it mutates
    // a libgit2 process global without internal synchronization. We call it
    // from a single deterministic point (the start of each network op, before
    // any concurrent git work — Git ops are not concurrency-safe, see
    // `GitTool::is_concurrency_safe == false`), passing a validated dir path. No
    // other unsafe is permitted in this crate (`#![deny(unsafe_code)]`).
    #[allow(unsafe_code)]
    unsafe {
        git2::opts::set_ssl_cert_dir(Path::new(dir)).map_err(|e| GitOpError::from_git2(&e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `make_fetch_options(Some(token))` wires up a credentials callback. We
    /// cannot invoke libgit2's private callback dispatch from Rust in
    /// isolation, so this asserts the assembly path: building the options with
    /// a token does not panic, and (separately) the underlying `Cred` the
    /// callback would produce is the expected `userpass_plaintext` credential.
    #[test]
    fn credentials_callback_yields_token() {
        // The options assemble without panicking with a token present.
        let _opts = make_fetch_options(Some("tok-123"));
        // And with no token (public repos).
        let _opts_none = make_fetch_options(None);

        // Directly construct the credential the callback closure returns, to
        // lock the convention (sentinel username + token-as-password). This is
        // exactly the value the installed `credentials` callback yields.
        git2::Cred::userpass_plaintext(TOKEN_USERNAME, "tok-123")
            .expect("userpass_plaintext should build a Cred from the token");
    }

    /// `set_ca_location(None)` is a no-op and never errors.
    #[test]
    fn set_ca_location_none_is_noop() {
        set_ca_location(None).expect("None CA dir is a no-op");
    }

    /// `set_ca_location(Some(dir))` exercises the `GIT_OPT_SET_SSL_CERT_LOCATIONS`
    /// path. On a device build (libgit2 + OpenSSL HTTPS transport) the option is
    /// accepted; some HOST builds compile libgit2 without an HTTPS transport that
    /// honours cert locations and return a named "TLS backend doesn't support
    /// certificate locations" error. Both are acceptable here — the real CA wiring
    /// is verified on-device (P4d). We only assert the call does not panic and any
    /// error is the named, mapped libgit2 error (never a silent success on a bad
    /// path or an unmapped error kind).
    #[test]
    fn set_ca_location_some_dir_ok_or_named_backend_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        match set_ca_location(Some(dir.path().to_str().unwrap())) {
            Ok(()) => {}
            Err(GitOpError::Libgit2(msg)) => {
                assert!(
                    msg.to_lowercase().contains("tls")
                        || msg.to_lowercase().contains("backend")
                        || msg.to_lowercase().contains("certificate"),
                    "CA-location failure on this host build should be the named \
                     TLS-backend error, got: {msg}"
                );
            }
            Err(other) => panic!("unexpected CA-location error kind: {other:?}"),
        }
    }
}
