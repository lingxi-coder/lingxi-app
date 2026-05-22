//! `lingxi-bridge` — IDE bridge protocol, pairing, and JWT verifier.
//!
//! This crate owns the engine-side half of the IDE bridge described in
//! spec §29. It exposes:
//!
//! - [`BridgeMessage`] — the 9-variant wire vocabulary (§29.2).
//! - [`IdeBridge`] — thin wrapper around any
//!   [`lingxi_traits::BridgeTransport`] implementation (§29.1).
//! - [`JwtVerifier`] — project-scoped HS256 JWT sign/verify (§29.3, A3).
//! - [`generate_pairing_code`] — 8-char human-readable pairing codes (A3).
//! - [`RateLimiter`] / [`BridgePairing`] — per-project pairing throttle (A4).
//!
//! Platform-specific transports (Tokio WebSocket etc.) live in `platforms/*`.

#![forbid(unsafe_code)]

pub mod codes;
pub mod jwt;
pub mod message;
pub mod pairing;
pub mod rate_limiter;
pub mod state;
pub mod transport;

pub use codes::generate_pairing_code;
pub use jwt::{JwtClaims, JwtError, JwtVerifier};
pub use message::BridgeMessage;
pub use pairing::{BridgePairing, PairingError, TrustedDevice};
pub use rate_limiter::RateLimiter;
pub use state::BridgeState;
pub use transport::IdeBridge;
