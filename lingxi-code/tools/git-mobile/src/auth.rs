//! In-process credential callback + CA wiring for the network git ops
//! (clone / fetch / pull), spec §G3 (auth) + §G6 (TLS/CA).
//!
//! The HTTPS token is supplied in-memory by the Kotlin host and installed into
//! libgit2's `RemoteCallbacks::credentials` only for the duration of a network
//! call (borrowed for the `FetchOptions` lifetime). It NEVER reaches disk, a
//! child process, argv, or an environment variable: libgit2 is in-process, so
//! there is no exec boundary to cross. The token is also never logged.
//!
//! ## TLS backend: mbedTLS (Android), CA store wiring
//!
//! libgit2 is built against **mbedTLS** for the Android target (the OpenSSL
//! Rust glue was removed in M-a). CA trust is configured at runtime by pointing
//! libgit2's mbedTLS stream at Android's system CA directory
//! (`/system/etc/security/cacerts`) via [`set_ca_location`]. This works because
//! this libgit2 version wires `GIT_OPT_SET_SSL_CERT_LOCATIONS` for the mbedTLS
//! backend too (not only OpenSSL): the option routes to
//! `git_mbedtls__set_cert_location(file, path)` in
//! `streams/mbedtls.c`, which loads a whole directory of hashed PEM certs with
//! `mbedtls_x509_crt_parse_path(path)` and installs them as the trust chain via
//! `mbedtls_ssl_conf_ca_chain`. No `streams/mbedtls.c` patch is required.
//!
//! Verification is **fail-closed (verify-required equivalent)**. The mbedTLS
//! config uses `MBEDTLS_SSL_VERIFY_OPTIONAL` *only* so libgit2 can still read
//! the peer certificate after the handshake (REQUIRED frees it on failure); the
//! stream's `verify_server_cert` then re-checks `mbedtls_ssl_get_verify_result`
//! and returns `GIT_ECERTIFICATE` on any failure. For HTTPS we install **no**
//! `certificate_check` callback (see [`make_network_callbacks`], which only adds
//! one for SSH host-key pinning), so libgit2's `httpclient.c` propagates that
//! `GIT_ECERTIFICATE` as a hard connection failure — there is no path that
//! accepts an unverified cert. This is identical to libgit2's OpenSSL backend
//! contract; we do NOT weaken the verify mode.
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
//! other unsafe can slip in. (Still required under mbedTLS — the setter remains
//! `unsafe` regardless of TLS backend.)

use std::path::{Path, PathBuf};

use crate::ops::GitOpError;

/// Username sent alongside the HTTPS token. GitHub/GitLab personal access
/// tokens are presented as the *password* with a fixed sentinel username; this
/// matches the documented `x-access-token` / PAT-as-password convention.
const TOKEN_USERNAME: &str = "x-access-token";

/// Install the HTTPS-token credentials callback on `callbacks`.
///
/// When `token` is `Some`, a `credentials` callback yields
/// `Cred::userpass_plaintext(TOKEN_USERNAME, token)` — the token is borrowed for
/// the callbacks' lifetime (`'a`), never copied into a longer-lived store. When
/// `None`, nothing is installed (public/anonymous HTTPS + `file://` still work).
/// The token is never logged or written anywhere; it only flows into libgit2's
/// in-process credential callback.
///
/// NOTE (G7): the live network ops now build their callbacks via
/// [`make_network_callbacks`] (which handles BOTH the HTTPS token and the SSH
/// key + host-key verification). This helper + [`make_fetch_options`] are
/// retained (HTTPS-only convenience, still unit-tested) but are no longer on the
/// clone/fetch/pull/push path; prefer `make_network_callbacks` for new callers.
pub fn install_token_credentials<'a>(
    callbacks: &mut git2::RemoteCallbacks<'a>,
    token: Option<&'a str>,
) {
    if let Some(token) = token {
        callbacks.credentials(move |_url, _username_from_url, _allowed| {
            git2::Cred::userpass_plaintext(TOKEN_USERNAME, token)
        });
    }
}

/// Build the [`git2::FetchOptions`] used by every network op (clone / fetch /
/// pull), with the in-process token credentials callback installed.
///
/// When `token` is `Some`, a `credentials` callback is installed that yields
/// `Cred::userpass_plaintext(TOKEN_USERNAME, token)` — the token is borrowed
/// for the returned `FetchOptions`' lifetime (`'a`), never copied into a
/// longer-lived store. When `token` is `None`, no credentials callback is
/// installed, so public (anonymous) HTTPS / `file://` remotes still work.
///
/// The token is never logged or written anywhere; it only flows into the
/// libgit2 credential callback in-process. See [`install_token_credentials`],
/// which holds the token convention shared with `ops::push`.
#[must_use]
pub fn make_fetch_options(token: Option<&str>) -> git2::FetchOptions<'_> {
    let mut callbacks = git2::RemoteCallbacks::new();
    install_token_credentials(&mut callbacks, token);
    let mut opts = git2::FetchOptions::new();
    opts.remote_callbacks(callbacks);
    opts
}

/// SSH-key authentication material for a network git op, supplied in-memory by
/// the Kotlin host (spec §G7). The private key is a **file path** (validated to
/// stay inside the app sandbox by [`validate_ssh_key_path`] before use); the
/// passphrase, when present, is held in memory and flows only into libgit2's
/// in-process credential callback (never logged, never written, never exec'd).
/// The host key is verified strictly: only fingerprints in
/// `known_hosts_sha256_hex` (lowercase-hex SHA-256) are accepted.
#[derive(Clone, Default)]
pub struct SshConfig {
    /// Filesystem path to the private key. Validated against the sandbox root
    /// (see [`validate_ssh_key_path`]) before being handed to libgit2.
    pub private_key_path: String,
    /// Optional path to the matching public key. libgit2/libssh2 can derive it
    /// from the private key when `None`.
    pub public_key_path: Option<String>,
    /// Optional passphrase decrypting the private key. In-memory only.
    pub passphrase: Option<String>,
    /// Pinned host-key fingerprints (lowercase-hex SHA-256). The remote's host
    /// key is accepted only if its SHA-256 is a member (compared
    /// case-insensitively); an empty list rejects every host key.
    pub known_hosts_sha256_hex: Vec<String>,
}

/// Hex-encode a host-key SHA-256 digest as a lowercase 64-char string.
///
/// libgit2 (`git2::Cert::as_hostkey().hash_sha256()`) hands us the raw 32-byte
/// SHA-256 of the remote's host key; we render it as lowercase hex to compare
/// against the host-supplied pinned `known_hosts_sha256_hex`.
#[must_use]
pub fn hostkey_sha256_hex(sha256: &[u8]) -> String {
    let mut out = String::with_capacity(sha256.len() * 2);
    for b in sha256 {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Strict host-key pinning check: is the remote's host-key SHA-256 in the
/// host-supplied pinned set?
///
/// `sha256` is the raw digest from the certificate; `pinned_hex` are
/// hex-encoded fingerprints (any case). Returns `true` only on an exact hex
/// match (compared case-insensitively). An empty `pinned_hex` rejects every
/// key — fail-closed.
#[must_use]
pub fn host_key_is_pinned(sha256: &[u8], pinned_hex: &[String]) -> bool {
    let actual = hostkey_sha256_hex(sha256);
    pinned_hex.iter().any(|p| p.eq_ignore_ascii_case(&actual))
}

/// Validate that an SSH private-key path stays inside the app sandbox.
///
/// Both `path` and `sandbox_root` are canonicalized (resolving `..`/symlinks),
/// then the key must lie under the canonical root — mirroring
/// [`crate::ops::open_repo`]'s containment check. A nonexistent key (or root)
/// fails to canonicalize and is rejected.
///
/// # Errors
///
/// - [`GitOpError::NotFound`] — `path` or `sandbox_root` cannot be canonicalized
///   (does not exist).
/// - [`GitOpError::InvalidInput`] — the canonical key path lies outside the
///   canonical sandbox root.
pub fn validate_ssh_key_path(
    path: &str,
    sandbox_root: &Path,
) -> Result<PathBuf, GitOpError> {
    let canonical_root = sandbox_root.canonicalize().map_err(|e| {
        GitOpError::NotFound(format!("sandbox root {}: {e}", sandbox_root.display()))
    })?;
    let canonical_key = Path::new(path)
        .canonicalize()
        .map_err(|e| GitOpError::NotFound(format!("ssh key {path}: {e}")))?;
    if !canonical_key.starts_with(&canonical_root) {
        return Err(GitOpError::InvalidInput(format!(
            "ssh key path {} escapes the app sandbox",
            canonical_key.display()
        )));
    }
    Ok(canonical_key)
}

/// Borrowed credential/host-key inputs for a single network op. Exactly one of
/// `token` (HTTPS) / `ssh` is expected in practice, but both being present is
/// handled (the credentials closure picks per the libgit2-requested type).
pub struct NetCallbacks<'a> {
    /// HTTPS token (PAT), presented as the password with the sentinel username.
    pub token: Option<&'a str>,
    /// SSH-key material + pinned host keys.
    pub ssh: Option<&'a SshConfig>,
}

/// Build the [`git2::RemoteCallbacks`] for a network op, installing a unified
/// `credentials` closure (HTTPS token and/or SSH key) and — when SSH is in play
/// — a strict `certificate_check` host-key verifier.
///
/// The `credentials` closure dispatches on the libgit2-requested
/// [`git2::CredentialType`] in order:
/// - `USERNAME` → [`git2::Cred::username`] (libssh2 asks for the username first
///   when none is in the URL).
/// - `SSH_KEY` (and `ssh` present) → [`git2::Cred::ssh_key`] with the validated
///   private-key path + optional public key + optional passphrase.
/// - `USER_PASS_PLAINTEXT` (and `token` present) → [`git2::Cred::userpass_plaintext`]
///   with the `x-access-token` sentinel.
/// - otherwise → an error (no usable credential).
///
/// The SSH username is taken from the URL (`username_from_url`) when libgit2
/// supplies it, else defaults to `git`.
///
/// When `ssh` is present, `certificate_check` reads the remote host key's
/// SHA-256 and returns [`git2::CertificateCheckStatus::CertificateOk`] iff it is
/// pinned in `known_hosts_sha256_hex`; an unknown/mismatched host key is a hard
/// error (fail-closed). A non-host-key certificate (e.g. an HTTPS X.509 cert,
/// which can't occur on an SSH transport) is passed through to libgit2's default
/// verification.
///
/// Secrets (token, passphrase, key bytes) flow only into libgit2 in-process and
/// are never logged.
#[must_use]
pub fn make_network_callbacks<'a>(p: &NetCallbacks<'a>) -> git2::RemoteCallbacks<'a> {
    let mut callbacks = git2::RemoteCallbacks::new();

    let token = p.token;
    let ssh = p.ssh;
    callbacks.credentials(move |_url, username_from_url, allowed| {
        let user = username_from_url.unwrap_or("git");
        if allowed.contains(git2::CredentialType::USERNAME) {
            return git2::Cred::username(user);
        }
        if let Some(ssh) = ssh {
            if allowed.contains(git2::CredentialType::SSH_KEY) {
                let public = ssh.public_key_path.as_deref().map(Path::new);
                return git2::Cred::ssh_key(
                    user,
                    public,
                    Path::new(&ssh.private_key_path),
                    ssh.passphrase.as_deref(),
                );
            }
        }
        if let Some(token) = token {
            if allowed.contains(git2::CredentialType::USER_PASS_PLAINTEXT) {
                return git2::Cred::userpass_plaintext(TOKEN_USERNAME, token);
            }
        }
        Err(git2::Error::from_str(
            "no usable git credential for the requested authentication type",
        ))
    });

    if let Some(ssh) = ssh {
        let pinned = ssh.known_hosts_sha256_hex.clone();
        callbacks.certificate_check(move |cert, _host| {
            let Some(hostkey) = cert.as_hostkey() else {
                // Not an SSH host key (e.g. an X.509 cert) — defer to libgit2's
                // default verification rather than vouching for it here.
                return Ok(git2::CertificateCheckStatus::CertificatePassthrough);
            };
            match hostkey.hash_sha256() {
                Some(sha256) if host_key_is_pinned(sha256, &pinned) => {
                    Ok(git2::CertificateCheckStatus::CertificateOk)
                }
                _ => Err(git2::Error::from_str(
                    "unknown or mismatched SSH host key (not in pinned known_hosts)",
                )),
            }
        });
    }

    callbacks
}

/// Build a [`git2::FetchOptions`] whose `RemoteCallbacks` come from
/// [`make_network_callbacks`] — i.e. the unified HTTPS-token + SSH-key
/// credentials closure plus (when SSH is in play) the strict host-key
/// `certificate_check`. This is the network-op fetch path for clone / fetch /
/// pull, so SSH and HTTPS both work through the same callbacks.
///
/// Secrets borrowed by the callbacks live for the returned options' lifetime
/// (`'a`); they only flow into libgit2 in-process and are never logged.
#[must_use]
pub fn make_fetch_options_net<'a>(p: &NetCallbacks<'a>) -> git2::FetchOptions<'a> {
    let mut opts = git2::FetchOptions::new();
    opts.remote_callbacks(make_network_callbacks(p));
    opts
}

/// Point libgit2's **mbedTLS** backend at a CA-certificate directory for
/// server-certificate verification.
///
/// `ca_dir`, when `Some`, is treated as a **directory** of one-cert-per-file CA
/// certificates (the Android `/system/etc/security/cacerts` layout: hashed
/// `<hash>.0` PEM files) and passed as the `path` argument of
/// `GIT_OPT_SET_SSL_CERT_LOCATIONS` via [`git2::opts::set_ssl_cert_dir`]; the
/// `file` argument is left unset. Under mbedTLS this option routes to
/// `git_mbedtls__set_cert_location(NULL, path)`, which loads the whole
/// directory with `mbedtls_x509_crt_parse_path` and installs it as the trust
/// chain (`mbedtls_ssl_conf_ca_chain`). Verification then fails closed against
/// that chain (see the module-level doc: `verify_server_cert` →
/// `GIT_ECERTIFICATE`, no HTTPS `certificate_check` override). When `None`, this
/// is a no-op — libgit2 keeps its built-in default (none), which is fine for the
/// host `file://` tests that perform no TLS.
///
/// This is a **global** libgit2 option (set once for the process). It is
/// idempotent to call again with the same value.
///
/// # Errors
///
/// [`GitOpError::Libgit2`] if libgit2/mbedTLS rejects the location (e.g. the
/// directory cannot be parsed into any valid cert).
pub fn set_ca_location(ca_dir: Option<&str>) -> Result<(), GitOpError> {
    let Some(dir) = ca_dir else {
        return Ok(());
    };
    // SAFETY: `git2::opts::set_ssl_cert_dir` is `unsafe` only because it mutates
    // a libgit2 process global without internal synchronization. We call it
    // from a single deterministic point (the start of each network op, before
    // any concurrent git work — Git ops are not concurrency-safe, see
    // `GitTool::is_concurrency_safe == false`), passing a validated dir path. It
    // drives the mbedTLS backend's `git_mbedtls__set_cert_location` in this
    // libgit2 version. No other unsafe is permitted in this crate
    // (`#![deny(unsafe_code)]`).
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

    #[test]
    fn install_token_credentials_is_noop_without_token() {
        // No token => no credentials callback installed; building options/callbacks
        // must not panic. (We cannot invoke libgit2's private dispatch in isolation.)
        let mut cb = git2::RemoteCallbacks::new();
        install_token_credentials(&mut cb, None);
        let mut cb2 = git2::RemoteCallbacks::new();
        install_token_credentials(&mut cb2, Some("tok-xyz"));
        // The token convention is still the userpass_plaintext sentinel:
        git2::Cred::userpass_plaintext(TOKEN_USERNAME, "tok-xyz")
            .expect("userpass_plaintext should build a Cred");
    }

    /// `set_ca_location(None)` is a no-op and never errors.
    #[test]
    fn set_ca_location_none_is_noop() {
        set_ca_location(None).expect("None CA dir is a no-op");
    }

    #[test]
    fn hostkey_hex_matches_pinned() {
        let raw = [0xABu8; 32];
        let hex = hostkey_sha256_hex(&raw);
        assert_eq!(hex.len(), 64);
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        let pinned = vec![hex.clone()];
        assert!(host_key_is_pinned(&raw, &pinned), "exact hex match accepted");
        let other = [0x00u8; 32];
        assert!(!host_key_is_pinned(&other, &pinned), "unknown key rejected");
        let pinned_upper = vec![hex.to_uppercase()];
        assert!(host_key_is_pinned(&raw, &pinned_upper), "pinned hex compared case-insensitively");
        // Fail-closed: an empty pinned set NEVER trusts a host key (no MITM defense
        // would otherwise be bypassed by an unconfigured known_hosts).
        assert!(!host_key_is_pinned(&raw, &[]), "empty pinned list must reject (fail-closed)");
    }

    #[test]
    fn validate_ssh_key_path_rejects_outside_sandbox() {
        let sandbox = tempfile::tempdir().unwrap();
        let key = sandbox.path().join("id_ed25519");
        std::fs::write(&key, b"-----BEGIN OPENSSH PRIVATE KEY-----\n").unwrap();
        validate_ssh_key_path(key.to_str().unwrap(), sandbox.path())
            .expect("in-sandbox key path accepted");
        let missing = sandbox.path().join("nope");
        assert!(validate_ssh_key_path(missing.to_str().unwrap(), sandbox.path()).is_err());
        let outside = sandbox.path().join("../escape");
        assert!(validate_ssh_key_path(outside.to_str().unwrap(), sandbox.path()).is_err());
    }

    #[test]
    fn make_network_callbacks_assembles_for_https_and_ssh() {
        let _cb = make_network_callbacks(&NetCallbacks { token: Some("tok"), ssh: None });
        let ssh = SshConfig {
            private_key_path: "/sandbox/id".into(),
            known_hosts_sha256_hex: vec!["abc".into()],
            ..Default::default()
        };
        let _cb2 = make_network_callbacks(&NetCallbacks { token: None, ssh: Some(&ssh) });
    }

    /// `set_ca_location(Some(dir))` exercises the `GIT_OPT_SET_SSL_CERT_LOCATIONS`
    /// path, which under the mbedTLS backend routes to
    /// `git_mbedtls__set_cert_location` (loads the dir via
    /// `mbedtls_x509_crt_parse_path`). The host can't exercise the Android trust
    /// store, but the reworked contract must be sane: calling it never panics and
    /// returns either `Ok(())` or a NAMED [`GitOpError::Libgit2`] — never a silent
    /// wrong-success of some other error kind, and never a weakened/skipped verify
    /// (the verify-required-equivalent contract is enforced inside libgit2, see the
    /// module doc; the live good-vs-bad-cert check is device-only, M-c). Whether an
    /// empty tempdir yields Ok or a named error depends on how mbedTLS/libgit2 is
    /// built on this host — both are acceptable; we only pin the contract shape.
    #[test]
    fn set_ca_location_contract_under_mbedtls() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Calling must not panic, and the only acceptable outcomes are Ok(()) or
        // a NAMED `GitOpError::Libgit2(_)` — never a silent wrong-success of some
        // other error kind.
        let acceptable = matches!(
            set_ca_location(Some(dir.path().to_str().unwrap())),
            Ok(()) | Err(GitOpError::Libgit2(_))
        );
        assert!(
            acceptable,
            "set_ca_location must return Ok or the named GitOpError::Libgit2, \
             never an unexpected error kind"
        );
    }
}
