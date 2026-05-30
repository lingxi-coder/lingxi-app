//! Platform-default [`SecureStorage`] factory.
//!
//! On macOS, tries [`super::macos::MacOsKeychainStorage`] first. On any init
//! error (e.g. `security` CLI absent on a non-default macOS, or a sandboxed
//! runtime that blocks subprocess spawn), logs the documented warning and
//! falls back to [`super::plaintext::PlainTextSecureStorage`].
//!
//! On Linux, returns plaintext directly with a
//! `// TODO: add libsecret support for Linux` placeholder — matches
//! claude-code's Linux behavior (`auth.ts` falls back to plaintext under the
//! same comment).

use std::path::PathBuf;
use std::sync::Arc;
use traits::{SecureStorage, SecureStorageError};

/// Return the best available [`SecureStorage`] for the current OS.
///
/// `user` is the keychain account name (claude-code uses `$USER`).
/// `config_dir` is the user's claude config directory (`~/.claude` or the
/// `CLAUDE_CONFIG_DIR`-overridden path).
/// `plaintext_path` is the fallback file location — typically
/// `<config_dir>/.credentials.json`.
///
/// On macOS, the macOS keychain backend is attempted first. The plaintext
/// fallback emits the warning
/// `"Warning: Storing credentials in plaintext."` exactly once at
/// `tracing::warn!` level.
///
/// # Errors
/// Returns [`SecureStorageError::Io`] when the plaintext fallback cannot
/// create its base directory (typically a permission issue on `config_dir`).
pub async fn secure_storage_for_platform(
    user: String,
    config_dir: PathBuf,
    plaintext_path: PathBuf,
) -> Result<Arc<dyn SecureStorage>, SecureStorageError> {
    #[cfg(target_os = "macos")]
    {
        let default_dir = default_claude_dir();
        match super::macos::MacOsKeychainStorage::new(
            user.clone(),
            config_dir.clone(),
            default_dir,
            String::new(),
        ) {
            Ok(keychain) => return Ok(Arc::new(keychain)),
            Err(e) => {
                tracing::warn!(
                    target: "lingxi::secure_storage",
                    error = %e,
                    "Warning: Storing credentials in plaintext."
                );
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        // TODO: add libsecret support for Linux
        // (matches claude-code's comment — Linux falls back to plaintext
        //  with no further attempt.)
        tracing::warn!(
            target: "lingxi::secure_storage",
            "Warning: Storing credentials in plaintext."
        );
        let _ = &user;
        let _ = &config_dir;
    }
    let plain = super::plaintext::PlainTextSecureStorage::new(plaintext_path).await?;
    Ok(Arc::new(plain))
}

#[cfg(target_os = "macos")]
fn default_claude_dir() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".claude")
    } else {
        PathBuf::from("/.claude")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn factory_returns_storage_handle() {
        let dir = tempdir().expect("tempdir");
        let plain = dir.path().join("creds-base");
        let storage = secure_storage_for_platform("test".into(), dir.path().to_path_buf(), plain)
            .await
            .expect("factory");
        // is_encrypted is true on macOS keychain, false on plaintext — we
        // only check that the trait method dispatches.
        let _ = storage.is_encrypted();
    }
}
