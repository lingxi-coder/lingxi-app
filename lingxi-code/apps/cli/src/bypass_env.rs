//! `RealBypassEnv` — the production [`permission::bypass_guard::BypassEnv`]
//! impl: real `getuid`, `/.dockerenv`, process env, and a 1s HTTP HEAD probe
//! to `http://1.1.1.1` via the posix HTTP transport (`platform-posix`).
//!
//! NOTE on `has_internet`: the CLI binds the real `platform_posix::PosixHttp`
//! (`platforms/common` `ReqwestHttp`), so the HEAD probe performs a live
//! connectivity check. The only caller (`enforce_bypass_safety` check 2) is
//! behind the `USER_TYPE == "ant"` gate, which is never true in external
//! builds — so the internet sub-condition stays unreachable there — but the
//! probe is now faithful end-to-end when the ant path is exercised.

use std::sync::Arc;
use std::time::Duration;

use permission::bypass_guard::BypassEnv;
use platform_api::HttpTransport;
use protocol::transport::{HttpMethod, HttpRequest};

/// Production environment probe for the bypass safety guard.
pub struct RealBypassEnv {
    http: Arc<dyn HttpTransport>,
}

impl RealBypassEnv {
    /// Build with the posix HTTP transport for the internet probe.
    #[must_use]
    pub fn new() -> Self {
        Self {
            http: Arc::new(platform_posix::PosixHttp::new()),
        }
    }
}

impl Default for RealBypassEnv {
    fn default() -> Self {
        Self::new()
    }
}

/// `process.getuid()` — the REAL uid (parity with claude-code `setup.ts`, which
/// calls `process.getuid()`, NOT the effective uid). Under `sudo` both the real
/// and effective uid are `0`, so this only diverges from `geteuid()` for setuid
/// binaries; we match TS byte-for-byte. Non-unix hosts return a non-zero
/// sentinel so the root check (`== 0`) is a no-op.
///
/// Uses `nix::unistd::getuid()` — a SAFE wrapper around `getuid(2)` — so this
/// crate keeps its `#![forbid(unsafe_code)]` (a raw `libc::getuid()` would need
/// an `unsafe` block, which `forbid` cannot locally override).
#[cfg(unix)]
fn real_uid() -> u32 {
    nix::unistd::getuid().as_raw()
}

#[cfg(not(unix))]
fn real_uid() -> u32 {
    u32::MAX
}

#[async_trait::async_trait]
impl BypassEnv for RealBypassEnv {
    fn is_windows(&self) -> bool {
        cfg!(windows)
    }

    fn effective_uid(&self) -> u32 {
        // Parity note: the trait method is named `effective_uid` (its contract),
        // but claude-code calls `process.getuid()` (the REAL uid), so the impl
        // returns `getuid()`. See `real_uid` above.
        real_uid()
    }

    fn env(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }

    fn is_docker(&self) -> bool {
        cfg!(target_os = "linux") && std::path::Path::new("/.dockerenv").exists()
    }

    async fn has_internet(&self) -> bool {
        // TS: axios HEAD http://1.1.1.1, 1s timeout, any success ⇒ true.
        // `PosixHttp` is the production reqwest-backed transport; this remains
        // unreachable in normal external builds because the caller is ant-only.
        let req = HttpRequest {
            method: HttpMethod::Head,
            url: "http://1.1.1.1".to_string(),
            headers: Vec::new(),
            body: None,
            body_bytes: None,
            timeout: Some(Duration::from_secs(1)),
        };
        self.http.request(req).await.is_ok()
    }
}
