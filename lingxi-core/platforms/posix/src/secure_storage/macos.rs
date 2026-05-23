//! macOS Keychain backend for [`SecureStorage`], shelling out to the
//! `security` CLI.
//!
//! See `claude-code/src/utils/secureStorage/macOsKeychainStorage.ts` for the
//! reference implementation. Behavior locked here:
//! - Service name from [`super::helpers::full_service_name`].
//! - JSON payload hex-encoded into `security -i`'s stdin (`-X <hex>`), with
//!   a length-checked argv fallback when the command would overflow
//!   [`super::helpers::SECURITY_STDIN_LINE_LIMIT`].
//! - 30 s TTL read cache with generation counter (prevents stale subprocess
//!   results from overwriting fresh writes) and in-flight dedupe (concurrent
//!   reads share a single subprocess).
//! - `list` is not supported — claude-code's API doesn't expose prefix
//!   queries via `security`. Returns
//!   [`SecureStorageError::BackendUnavailable`] with a documented message.

use crate::secure_storage::helpers::{
    full_service_name, KEYCHAIN_CACHE_TTL, SECURITY_STDIN_LINE_LIMIT,
};
use async_trait::async_trait;
use lingxi_protocol::SecureStorageData;
use lingxi_traits::{SecureStorage, SecureStorageBackend, SecureStorageError};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::{Mutex, Notify, RwLock};

type CacheKey = (String, String);

#[derive(Clone)]
struct CachedEntry {
    data: SecureStorageData,
    fetched_at: Instant,
    generation: u64,
}

/// macOS Keychain backend.
///
/// Construct via [`MacOsKeychainStorage::new`]. The `user` field is the
/// `security -a <account>` argument (typically `$USER`); the `config_dir`
/// drives the per-config-directory dir-hash service-name suffix.
pub struct MacOsKeychainStorage {
    user: String,
    config_dir: PathBuf,
    default_config_dir: PathBuf,
    oauth_suffix: String,
    cache: Arc<RwLock<HashMap<CacheKey, CachedEntry>>>,
    generation: Arc<AtomicU64>,
    inflight: Arc<Mutex<HashMap<CacheKey, Arc<Notify>>>>,
}

impl MacOsKeychainStorage {
    /// Construct a new keychain-backed store.
    ///
    /// `user` is the keychain account name (claude-code uses
    /// `process.env.USER || userInfo().username`).
    /// `config_dir` is the user's claude config directory (`~/.claude` or
    /// whatever `CLAUDE_CONFIG_DIR` overrides it to).
    /// `default_config_dir` is what claude-code calls the "default"
    /// `~/.claude` — passed in so the dir-hash discriminator can compare.
    /// `oauth_suffix` mirrors claude-code's `OAUTH_FILE_SUFFIX`; pass `""`
    /// for the standard build.
    ///
    /// # Errors
    /// Returns [`SecureStorageError::BackendUnavailable`] when the `security`
    /// CLI is not on `$PATH` (the caller can then fall back to plaintext).
    pub fn new(
        user: String,
        config_dir: PathBuf,
        default_config_dir: PathBuf,
        oauth_suffix: String,
    ) -> Result<Self, SecureStorageError> {
        if which::which("security").is_err() {
            return Err(SecureStorageError::BackendUnavailable(
                "macOS `security` CLI not found on PATH".into(),
            ));
        }
        Ok(Self {
            user,
            config_dir,
            default_config_dir,
            oauth_suffix,
            cache: Arc::new(RwLock::new(HashMap::new())),
            generation: Arc::new(AtomicU64::new(0)),
            inflight: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Convenience constructor with the default `OAUTH_FILE_SUFFIX = ""`.
    ///
    /// # Errors
    /// Same as [`MacOsKeychainStorage::new`].
    pub fn new_default_oauth_suffix(
        user: String,
        config_dir: PathBuf,
        default_config_dir: PathBuf,
    ) -> Result<Self, SecureStorageError> {
        Self::new(user, config_dir, default_config_dir, String::new())
    }

    /// Return the full keychain service name for the supplied `service_suffix`.
    ///
    /// Callers pass `"-credentials"` (OAuth) or `""` (legacy API key) — the
    /// "Claude Code" prefix and `dir_hash` are interpolated here so the
    /// resulting string matches claude-code's literal layout.
    pub(crate) fn keychain_service_name(&self, service_suffix: &str) -> String {
        let dir_hash = super::helpers::compute_dir_hash(
            self.config_dir.as_path(),
            self.default_config_dir.as_path(),
        );
        full_service_name(
            "Claude Code",
            self.oauth_suffix.as_str(),
            service_suffix,
            &dir_hash,
        )
    }

    /// Bump the generation counter. Called on every successful store/delete
    /// and on explicit cache invalidation. A pending `retrieve` that
    /// observes a higher counter when its subprocess returns must NOT write
    /// its (now-stale) result to the cache. Matches claude-code's
    /// `keychainCacheState.generation`.
    pub(crate) fn bump_generation(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }
}

#[async_trait]
impl SecureStorage for MacOsKeychainStorage {
    async fn store(
        &self,
        service: &str,
        account: &str,
        data: SecureStorageData,
    ) -> Result<(), SecureStorageError> {
        // Pre-invalidate the cache so a concurrent reader doesn't return
        // stale data after we bump the generation but before the subprocess
        // returns.
        let key = (service.to_string(), account.to_string());
        self.cache.write().await.remove(&key);
        self.bump_generation();

        let full_service = self.keychain_service_name(service);

        // JSON → hex. Claude-code uses
        // `Buffer.from(jsonString, 'utf-8').toString('hex')`.
        let json = serde_json::to_string(&data)
            .map_err(|e| SecureStorageError::Io(format!("serialize: {e}")))?;
        let hex_value = hex::encode(json.as_bytes());

        // Preferred path: `security -i` reads the full add-generic-password
        // command on stdin. Keeps the hex payload out of argv so process
        // monitors only see `security -i`.
        let stdin_command = format!(
            "add-generic-password -U -a \"{}\" -s \"{}\" -X \"{}\"\n",
            self.user.as_str(),
            full_service,
            hex_value,
        );

        let (exit_status, stderr) = if stdin_command.len() <= SECURITY_STDIN_LINE_LIMIT {
            run_security_stdin(&stdin_command).await?
        } else {
            // Argv fallback. Hex in argv is recoverable by a determined
            // observer but defeats naive plaintext-grep rules — silent
            // credential corruption (the alternative) is strictly worse.
            tracing::warn!(
                target: "lingxi::secure_storage::macos",
                "Keychain payload ({} B JSON) exceeds security -i stdin limit; using argv",
                json.len()
            );
            run_security_argv(&[
                "add-generic-password",
                "-U",
                "-a",
                self.user.as_str(),
                "-s",
                full_service.as_str(),
                "-X",
                hex_value.as_str(),
            ])
            .await?
        };

        if !exit_status.success() {
            return Err(SecureStorageError::BackendUnavailable(format!(
                "security add-generic-password exited {}: {}",
                exit_status.code().unwrap_or(-1),
                stderr.trim()
            )));
        }

        // Cache the freshly-written data with the new generation.
        let now = Instant::now();
        let gen = self.generation.load(Ordering::Acquire);
        self.cache.write().await.insert(
            key,
            CachedEntry {
                data,
                fetched_at: now,
                generation: gen,
            },
        );
        Ok(())
    }

    async fn retrieve(
        &self,
        service: &str,
        account: &str,
    ) -> Result<Option<SecureStorageData>, SecureStorageError> {
        let key = (service.to_string(), account.to_string());
        let now = Instant::now();
        let cur_gen = self.generation.load(Ordering::Acquire);

        // Cache hit fast path.
        if let Some(entry) = self.cache.read().await.get(&key).cloned() {
            if entry.generation == cur_gen
                && now.duration_since(entry.fetched_at) < KEYCHAIN_CACHE_TTL
            {
                return Ok(Some(entry.data));
            }
        }

        // In-flight dedupe: if another retrieve is already running for this
        // key, await its Notify and retry the cache read.
        let notify = {
            let mut inflight = self.inflight.lock().await;
            if let Some(existing) = inflight.get(&key).cloned() {
                drop(inflight);
                existing.notified().await;
                // Re-read the cache after the other call completed.
                if let Some(entry) = self.cache.read().await.get(&key).cloned() {
                    return Ok(Some(entry.data));
                }
                return Ok(None);
            }
            let n = Arc::new(Notify::new());
            inflight.insert(key.clone(), n.clone());
            n
        };

        // We are the responsible spawner.
        let full_service = self.keychain_service_name(service);
        let result = run_security_find(&self.user, &full_service).await;

        // Notify waiters regardless of outcome and remove our inflight entry.
        {
            let mut inflight = self.inflight.lock().await;
            inflight.remove(&key);
        }
        notify.notify_waiters();

        match result {
            Ok(Some(data)) => {
                // Generation check: if the counter changed during our
                // subprocess, our result is stale — return it to *this*
                // caller but do NOT poison the cache.
                let post_gen = self.generation.load(Ordering::Acquire);
                if post_gen == cur_gen {
                    self.cache.write().await.insert(
                        key,
                        CachedEntry {
                            data: data.clone(),
                            fetched_at: Instant::now(),
                            generation: post_gen,
                        },
                    );
                }
                Ok(Some(data))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn delete(&self, service: &str, account: &str) -> Result<(), SecureStorageError> {
        let key = (service.to_string(), account.to_string());
        // Invalidate cache + bump generation BEFORE the subprocess so any
        // racing retrieve sees the bump and discards its result.
        self.cache.write().await.remove(&key);
        self.bump_generation();

        let full_service = self.keychain_service_name(service);
        let output = Command::new("security")
            .args([
                "delete-generic-password",
                "-a",
                self.user.as_str(),
                "-s",
                full_service.as_str(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| SecureStorageError::Io(format!("spawn security: {e}")))?;

        // Treat "not found" (exit 44) as success — matches claude-code's
        // `try { ... } catch { return false }` semantics where the absence
        // is not an error from the caller's perspective.
        if output.status.success() || output.status.code() == Some(44) {
            return Ok(());
        }
        Err(SecureStorageError::BackendUnavailable(format!(
            "security delete-generic-password exited {}: {}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr)
        )))
    }

    async fn list(&self, _service: &str) -> Result<Vec<String>, SecureStorageError> {
        Err(SecureStorageError::BackendUnavailable(
            "list not supported on macOS Keychain backend".into(),
        ))
    }

    fn is_encrypted(&self) -> bool {
        true
    }

    fn backend(&self) -> SecureStorageBackend {
        SecureStorageBackend::MacOsKeychain
    }
}

/// Spawn `security -i` and feed the full command on stdin. Returns the exit
/// status and captured stderr so the caller can decorate errors.
async fn run_security_stdin(
    command: &str,
) -> Result<(std::process::ExitStatus, String), SecureStorageError> {
    let mut child = Command::new("security")
        .arg("-i")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| SecureStorageError::Io(format!("spawn security -i: {e}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(command.as_bytes())
            .await
            .map_err(|e| SecureStorageError::Io(format!("stdin write: {e}")))?;
        stdin
            .shutdown()
            .await
            .map_err(|e| SecureStorageError::Io(format!("stdin shutdown: {e}")))?;
    }
    let output = child
        .wait_with_output()
        .await
        .map_err(|e| SecureStorageError::Io(format!("wait: {e}")))?;
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    Ok((output.status, stderr))
}

/// Argv fallback for `security add-generic-password`.
async fn run_security_argv(
    args: &[&str],
) -> Result<(std::process::ExitStatus, String), SecureStorageError> {
    let output = Command::new("security")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| SecureStorageError::Io(format!("spawn security: {e}")))?;
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    Ok((output.status, stderr))
}

/// `security find-generic-password -a <user> -w -s <service>`.
///
/// Returns `Ok(None)` for `errSecItemNotFound` (exit 44) and `Ok(Some(_))`
/// on success. The password is read as the JSON-encoded
/// [`SecureStorageData`] (claude-code writes the JSON bytes via `-X <hex>`;
/// `security -w` prints the decoded bytes back as a UTF-8 string).
async fn run_security_find(
    user: &str,
    full_service: &str,
) -> Result<Option<SecureStorageData>, SecureStorageError> {
    let output = Command::new("security")
        .args([
            "find-generic-password",
            "-a",
            user,
            "-w",
            "-s",
            full_service,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| SecureStorageError::Io(format!("spawn security: {e}")))?;
    if !output.status.success() {
        // exit code 44 (errSecItemNotFound) is a normal "no entry" result.
        if output.status.code() == Some(44) {
            return Ok(None);
        }
        return Err(SecureStorageError::BackendUnavailable(format!(
            "security find-generic-password exited {}: {}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        return Ok(None);
    }
    let data: SecureStorageData = serde_json::from_str(&stdout)
        .map_err(|e| SecureStorageError::Io(format!("deserialize keychain payload: {e}")))?;
    Ok(Some(data))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_test_storage(cfg: &str, default: &str) -> MacOsKeychainStorage {
        MacOsKeychainStorage {
            user: "tester".into(),
            config_dir: PathBuf::from(cfg),
            default_config_dir: PathBuf::from(default),
            oauth_suffix: String::new(),
            cache: Arc::new(RwLock::new(HashMap::new())),
            generation: Arc::new(AtomicU64::new(0)),
            inflight: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    #[test]
    fn service_name_default_dir_has_no_dir_hash() {
        let s = mk_test_storage("/Users/x/.claude", "/Users/x/.claude");
        assert_eq!(
            s.keychain_service_name("-credentials"),
            "Claude Code-credentials"
        );
        assert_eq!(s.keychain_service_name(""), "Claude Code");
    }

    #[test]
    fn service_name_non_default_dir_has_dir_hash() {
        let s = mk_test_storage("/Users/x/work/.claude-2", "/Users/x/.claude");
        let svc = s.keychain_service_name("-credentials");
        assert!(svc.starts_with("Claude Code-credentials-"));
        assert_eq!(svc.len(), "Claude Code-credentials-".len() + 8);
    }

    #[test]
    fn bump_generation_increments() {
        let s = mk_test_storage("/a", "/b");
        let g0 = s.generation.load(Ordering::Acquire);
        s.bump_generation();
        let g1 = s.generation.load(Ordering::Acquire);
        assert_eq!(g1, g0 + 1);
    }
}
