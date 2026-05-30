//! Secure storage backends for desktop hosts.
//!
//! [`PlainTextSecureStorage`] (Linux + fallback) and [`MacOsKeychainStorage`]
//! (macOS, via the `security` CLI) implement
//! [`traits::SecureStorage`]. The
//! [`secure_storage_for_platform`] helper picks the best backend per OS,
//! with a documented plaintext-fallback warning when the preferred backend
//! cannot initialise.

pub mod factory;
pub mod helpers;
pub mod macos;
pub mod plaintext;

pub use factory::secure_storage_for_platform;
pub use helpers::{
    compute_dir_hash, full_service_name, CREDENTIALS_SERVICE_SUFFIX, KEYCHAIN_CACHE_TTL,
    SECURITY_STDIN_LINE_LIMIT,
};
pub use macos::MacOsKeychainStorage;
pub use plaintext::PlainTextSecureStorage;
