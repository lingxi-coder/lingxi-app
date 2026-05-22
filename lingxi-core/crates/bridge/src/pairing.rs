//! Pairing flow for trusted IDE devices.
//!
//! Pairing produces an 8-character code (see [`crate::codes`]) that the user
//! enters on the IDE side. The CLI side gates code generation behind a token
//! bucket keyed by `project_dir` (A4), so brute-forcing the small code space
//! is slowed to a crawl. After the IDE proves it knows the code, the CLI
//! mints a project-scoped JWT (see [`crate::jwt`]) and stores the device in
//! [`SecureStorage`].

use crate::codes::generate_pairing_code;
use crate::rate_limiter::RateLimiter;
use lingxi_traits::SecureStorage;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::SystemTime;
use thiserror::Error;

/// Errors raised during the pairing handshake.
#[derive(Debug, Clone, Error)]
pub enum PairingError {
    /// Pairing throttled — too many attempts for this project.
    #[error("rate limited")]
    RateLimited,
    /// The pairing code presented by the IDE is no longer valid.
    #[error("expired pairing code")]
    Expired,
    /// The pairing code did not match.
    #[error("invalid code")]
    Invalid,
    /// Secure storage failed during the pairing flow.
    #[error("storage: {0}")]
    Storage(String),
}

/// Persistent record describing a device that completed pairing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustedDevice {
    /// Stable device identifier (IDE-supplied).
    pub device_id: String,
    /// User-visible device name.
    pub name: String,
    /// First successful pairing time.
    pub paired_at: SystemTime,
    /// Last time we saw this device connect.
    pub last_seen: SystemTime,
    /// Project directory this device is paired to.
    pub project_dir: String,
}

/// State machine for the pairing handshake.
///
/// `storage` is held for future use — M1 only owns the rate limiter, and
/// the full persistence flow lands in a later milestone.
pub struct BridgePairing {
    #[allow(dead_code)]
    storage: Arc<dyn SecureStorage>,
    pairing_codes_rate_limiter: RateLimiter,
}

impl BridgePairing {
    /// Construct a pairing manager with a 3-attempt burst budget and a
    /// 1-per-minute refill. The budget is per project dir.
    #[must_use]
    pub fn new(storage: Arc<dyn SecureStorage>) -> Self {
        Self {
            storage,
            // 3 attempts burst, refill 1/min — slows pairing brute force.
            pairing_codes_rate_limiter: RateLimiter::new(3, 1.0 / 60.0),
        }
    }

    /// Generate a fresh pairing code for `project_dir`.
    ///
    /// Returns [`PairingError::RateLimited`] when the per-project budget is
    /// exhausted.
    pub fn generate_pairing_code(&self, project_dir: &str) -> Result<String, PairingError> {
        if !self.pairing_codes_rate_limiter.try_acquire(project_dir) {
            return Err(PairingError::RateLimited);
        }
        Ok(generate_pairing_code())
    }
}
