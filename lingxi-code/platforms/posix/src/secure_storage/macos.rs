//! macOS Keychain backend for [`SecureStorage`]. Production calls Apple's
//! Keychain Services (`SecItemAdd` / `SecItemUpdate` /
//! `SecItemCopyMatching` / `SecItemDelete`) through `security-framework`.
//! The API and ACL behavior follow Apple's Keychain Services documentation:
//! <https://developer.apple.com/documentation/security/keychain-services>.
//!
//! See `claude-code/src/utils/secureStorage/macOsKeychainStorage.ts` for the
//! reference implementation. Behavior locked here:
//! - Service name from [`super::helpers::full_service_name`].
//! - JSON payload stored as the generic-password value under the stable
//!   `(kSecAttrService, kSecAttrAccount)` identity.
//! - 30 s TTL read cache with generation counter (prevents stale subprocess
//!   results from overwriting fresh writes) and in-flight dedupe (concurrent
//!   reads share one Keychain Services query).
//! - `list` is not supported — claude-code's API doesn't expose prefix
//!   queries via `security`. Returns
//!   [`SecureStorageError::BackendUnavailable`] with a documented message.

use crate::secure_storage::helpers::{
    full_service_name, KEYCHAIN_CACHE_TTL, SECURITY_STDIN_LINE_LIMIT,
};
use async_trait::async_trait;
use platform_api::{SecureStorage, SecureStorageBackend, SecureStorageError};
use protocol::SecureStorageData;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::{Mutex, Notify, RwLock};

type CacheKey = (String, String);
const SECURITY_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone)]
struct CachedEntry {
    data: SecureStorageData,
    fetched_at: Instant,
    generation: u64,
}

/// macOS Keychain backend.
///
/// Construct via [`MacOsKeychainStorage::new`]. The `user` field is retained for
/// backend identity/diagnostics; individual [`SecureStorage`] calls use their
/// logical `account` argument as the `security -a <account>` lookup key so the
/// `(service, account)` contract is preserved. The `config_dir` drives the
/// per-config-directory dir-hash service-name suffix.
pub struct MacOsKeychainStorage {
    #[allow(dead_code)]
    user: String,
    config_dir: PathBuf,
    default_config_dir: PathBuf,
    oauth_suffix: String,
    /// Tests inject a fake `security` executable to keep Keychain tests
    /// hermetic. Production always uses Keychain Services directly.
    security_command: Option<PathBuf>,
    cache: Arc<RwLock<HashMap<CacheKey, CachedEntry>>>,
    generation: Arc<AtomicU64>,
    inflight: Arc<Mutex<HashMap<CacheKey, Arc<InflightLookup>>>>,
}

struct InflightLookup {
    notify: Notify,
    result: Mutex<Option<Result<Option<SecureStorageData>, SecureStorageError>>>,
}

impl MacOsKeychainStorage {
    /// Construct a new keychain-backed store.
    ///
    /// `user` is the keychain account name (claude-code uses
    /// `process.env.USER || userInfo().username`).
    /// `config_dir` is the user's claude config directory (`~/.claude` or
    /// whatever `LINGXI_CONFIG_DIR` overrides it to).
    /// `default_config_dir` is what claude-code calls the "default"
    /// `~/.claude` — passed in so the dir-hash discriminator can compare.
    /// `oauth_suffix` mirrors claude-code's `OAUTH_FILE_SUFFIX`; pass `""`
    /// for the standard build.
    ///
    /// # Errors
    /// Construction is infallible. Keychain Services errors are reported by
    /// the individual storage operation with their OSStatus code.
    pub fn new(
        user: String,
        config_dir: PathBuf,
        default_config_dir: PathBuf,
        oauth_suffix: String,
    ) -> Result<Self, SecureStorageError> {
        Ok(Self {
            user,
            config_dir,
            default_config_dir,
            oauth_suffix,
            security_command: None,
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
    /// Callers pass `"-credentials"` (OAuth) or `""` (API key) — the
    /// "LingXi" prefix and `dir_hash` are interpolated here so the
    /// resulting string matches claude-code's literal layout.
    pub(crate) fn keychain_service_name(&self, service_suffix: &str) -> String {
        let dir_hash = super::helpers::compute_dir_hash(
            self.config_dir.as_path(),
            self.default_config_dir.as_path(),
        );
        full_service_name(
            "LingXi",
            self.oauth_suffix.as_str(),
            service_suffix,
            &dir_hash,
        )
    }

    /// Bump the generation counter. Called on every successful store/delete
    /// and on explicit cache invalidation. A pending `retrieve` that
    /// observes a higher counter when its Keychain query returns must NOT write
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
        // stale data after we bump the generation but before the native call
        // returns.
        let key = (service.to_string(), account.to_string());
        self.cache.write().await.remove(&key);
        self.bump_generation();

        let full_service = self.keychain_service_name(service);

        let json = serde_json::to_vec(&data)
            .map_err(|e| SecureStorageError::Io(format!("serialize: {e}")))?;

        if let Some(security_command) = self.security_command.as_deref() {
            // Hermetic test path: fake the historical CLI adapter without
            // touching the developer's real login Keychain.
            let hex_value = hex::encode(&json);
            let stdin_command = format!(
                "add-generic-password -U -a \"{account}\" -s \"{full_service}\" -X \"{hex_value}\" -T \"/usr/bin/security\"\n",
            );
            let (exit_status, stderr) = if stdin_command.len() <= SECURITY_STDIN_LINE_LIMIT {
                run_security_stdin(security_command, &stdin_command).await?
            } else {
                run_security_argv(
                    security_command,
                    &[
                        "add-generic-password",
                        "-U",
                        "-a",
                        account,
                        "-s",
                        full_service.as_str(),
                        "-X",
                        hex_value.as_str(),
                        "-T",
                        "/usr/bin/security",
                    ],
                )
                .await?
            };
            if !exit_status.success() {
                return Err(SecureStorageError::BackendUnavailable(format!(
                    "security add-generic-password exited {}: {}",
                    exit_status.code().unwrap_or(-1),
                    stderr.trim()
                )));
            }
        } else {
            native_keychain_store(full_service, account.to_string(), json).await?;
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

        // In-flight dedupe: every waiter shares the leader's exact result,
        // including missing values and backend errors. Register the
        // notification future before checking the result so completion
        // cannot be lost between the check and the await.
        let (lookup, is_leader) = {
            let mut inflight = self.inflight.lock().await;
            if let Some(existing) = inflight.get(&key).cloned() {
                (existing, false)
            } else {
                let lookup = Arc::new(InflightLookup {
                    notify: Notify::new(),
                    result: Mutex::new(None),
                });
                inflight.insert(key.clone(), lookup.clone());
                (lookup, true)
            }
        };

        if !is_leader {
            loop {
                let notified = lookup.notify.notified();
                if let Some(result) = lookup.result.lock().await.clone() {
                    return result;
                }
                notified.await;
            }
        }

        // We own this in-flight Keychain lookup.
        let full_service = self.keychain_service_name(service);
        let fetched = if let Some(security_command) = self.security_command.as_deref() {
            run_security_find(security_command, account, &full_service).await
        } else {
            native_keychain_retrieve(full_service, account.to_string()).await
        };
        let result = match fetched {
            Ok(Some(data)) => {
                // Generation check: if the counter changed during our
                // subprocess, our result is stale — return it to *this*
                // caller but do NOT poison the cache.
                let post_gen = self.generation.load(Ordering::Acquire);
                if post_gen == cur_gen {
                    self.cache.write().await.insert(
                        key.clone(),
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
        };

        *lookup.result.lock().await = Some(result.clone());
        self.inflight.lock().await.remove(&key);
        lookup.notify.notify_waiters();
        result
    }

    async fn contains(&self, service: &str, account: &str) -> Result<bool, SecureStorageError> {
        if self.security_command.is_some() {
            return self
                .retrieve(service, account)
                .await
                .map(|entry| entry.is_some());
        }
        native_keychain_contains(self.keychain_service_name(service), account.to_string()).await
    }

    async fn delete(&self, service: &str, account: &str) -> Result<(), SecureStorageError> {
        let key = (service.to_string(), account.to_string());
        // Invalidate cache + bump generation BEFORE the native call so any
        // racing retrieve sees the bump and discards its result.
        self.cache.write().await.remove(&key);
        self.bump_generation();

        let full_service = self.keychain_service_name(service);
        if self.security_command.is_none() {
            return native_keychain_delete(full_service, account.to_string()).await;
        }
        let mut command = Command::new(self.security_command.as_deref().expect("checked above"));
        command
            .args([
                "delete-generic-password",
                "-a",
                account,
                "-s",
                full_service.as_str(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let output = tokio::time::timeout(SECURITY_COMMAND_TIMEOUT, command.output())
            .await
            .map_err(|_| {
                SecureStorageError::BackendUnavailable(
                    "security delete-generic-password timed out".to_string(),
                )
            })?
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

#[cfg(target_os = "macos")]
fn native_keychain_error(
    action: &'static str,
    error: security_framework::base::Error,
) -> SecureStorageError {
    let detail = format!(
        "Keychain Services {action} failed (OSStatus {}): {error}",
        error.code()
    );
    match error.code() {
        // errSecAuthFailed and userCanceled are explicit authorization
        // outcomes; the caller may choose a non-native fallback.
        -25293 | -128 => SecureStorageError::PermissionDenied(detail),
        _ => SecureStorageError::BackendUnavailable(detail),
    }
}

#[cfg(target_os = "macos")]
async fn native_keychain_store(
    service: String,
    account: String,
    payload: Vec<u8>,
) -> Result<(), SecureStorageError> {
    tokio::task::spawn_blocking(move || {
        security_framework::passwords::set_generic_password(&service, &account, &payload)
            .map_err(|error| native_keychain_error("store", error))
    })
    .await
    .map_err(|error| SecureStorageError::Io(format!("Keychain Services store task: {error}")))?
}

#[cfg(not(target_os = "macos"))]
async fn native_keychain_store(
    _service: String,
    _account: String,
    _payload: Vec<u8>,
) -> Result<(), SecureStorageError> {
    Err(SecureStorageError::BackendUnavailable(
        "Keychain Services are only available on macOS".to_string(),
    ))
}

#[cfg(target_os = "macos")]
async fn native_keychain_retrieve(
    service: String,
    account: String,
) -> Result<Option<SecureStorageData>, SecureStorageError> {
    let payload = tokio::task::spawn_blocking(move || {
        security_framework::passwords::get_generic_password(&service, &account)
    })
    .await
    .map_err(|error| SecureStorageError::Io(format!("Keychain Services retrieve task: {error}")))?;
    let payload = match payload {
        Ok(payload) => payload,
        // errSecItemNotFound is a normal absence, not a backend failure.
        Err(error) if error.code() == -25300 => return Ok(None),
        Err(error) => return Err(native_keychain_error("retrieve", error)),
    };
    serde_json::from_slice(&payload)
        .map(Some)
        .map_err(|error| SecureStorageError::Io(format!("deserialize Keychain payload: {error}")))
}

#[cfg(target_os = "macos")]
async fn native_keychain_contains(
    service: String,
    account: String,
) -> Result<bool, SecureStorageError> {
    tokio::task::spawn_blocking(move || {
        let mut query = security_framework::item::ItemSearchOptions::new();
        query
            .class(security_framework::item::ItemClass::generic_password())
            .service(&service)
            .account(&account);
        match query.search() {
            // No return-data or return-attributes flag is set: a successful
            // SecItemCopyMatching proves existence without decrypting the
            // secret and therefore does not trigger the password-read ACL.
            Ok(_) => Ok(true),
            Err(error) if error.code() == -25300 => Ok(false),
            Err(error) => Err(native_keychain_error("contains", error)),
        }
    })
    .await
    .map_err(|error| SecureStorageError::Io(format!("Keychain Services contains task: {error}")))?
}

#[cfg(not(target_os = "macos"))]
async fn native_keychain_contains(
    _service: String,
    _account: String,
) -> Result<bool, SecureStorageError> {
    Err(SecureStorageError::BackendUnavailable(
        "Keychain Services are only available on macOS".to_string(),
    ))
}

#[cfg(not(target_os = "macos"))]
async fn native_keychain_retrieve(
    _service: String,
    _account: String,
) -> Result<Option<SecureStorageData>, SecureStorageError> {
    Err(SecureStorageError::BackendUnavailable(
        "Keychain Services are only available on macOS".to_string(),
    ))
}

#[cfg(target_os = "macos")]
async fn native_keychain_delete(
    service: String,
    account: String,
) -> Result<(), SecureStorageError> {
    let result = tokio::task::spawn_blocking(move || {
        security_framework::passwords::delete_generic_password(&service, &account)
    })
    .await
    .map_err(|error| SecureStorageError::Io(format!("Keychain Services delete task: {error}")))?;
    match result {
        Ok(()) => Ok(()),
        Err(error) if error.code() == -25300 => Ok(()),
        Err(error) => Err(native_keychain_error("delete", error)),
    }
}

#[cfg(not(target_os = "macos"))]
async fn native_keychain_delete(
    _service: String,
    _account: String,
) -> Result<(), SecureStorageError> {
    Err(SecureStorageError::BackendUnavailable(
        "Keychain Services are only available on macOS".to_string(),
    ))
}

/// Spawn `security -i` and feed the full command on stdin. Returns the exit
/// status and captured stderr so the caller can decorate errors.
async fn run_security_stdin(
    security_command: &std::path::Path,
    command: &str,
) -> Result<(std::process::ExitStatus, String), SecureStorageError> {
    let mut process = Command::new(security_command);
    process
        .arg("-i")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = process
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
    let output = tokio::time::timeout(SECURITY_COMMAND_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| SecureStorageError::BackendUnavailable("security -i timed out".to_string()))?
        .map_err(|e| SecureStorageError::Io(format!("wait: {e}")))?;
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    Ok((output.status, stderr))
}

/// Argv fallback for `security add-generic-password`.
async fn run_security_argv(
    security_command: &std::path::Path,
    args: &[&str],
) -> Result<(std::process::ExitStatus, String), SecureStorageError> {
    let mut command = Command::new(security_command);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = tokio::time::timeout(SECURITY_COMMAND_TIMEOUT, command.output())
        .await
        .map_err(|_| {
            SecureStorageError::BackendUnavailable("security command timed out".to_string())
        })?
        .map_err(|e| SecureStorageError::Io(format!("spawn security: {e}")))?;
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    Ok((output.status, stderr))
}

/// `security find-generic-password -a <account> -w -s <service>`.
///
/// Returns `Ok(None)` for `errSecItemNotFound` (exit 44) and `Ok(Some(_))`
/// on success. The password is read as the JSON-encoded
/// [`SecureStorageData`] (claude-code writes the JSON bytes via `-X <hex>`;
/// `security -w` prints the decoded bytes back as a UTF-8 string).
async fn run_security_find(
    security_command: &std::path::Path,
    account: &str,
    full_service: &str,
) -> Result<Option<SecureStorageData>, SecureStorageError> {
    let mut command = Command::new(security_command);
    command
        .args([
            "find-generic-password",
            "-a",
            account,
            "-w",
            "-s",
            full_service,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = tokio::time::timeout(SECURITY_COMMAND_TIMEOUT, command.output())
        .await
        .map_err(|_| {
            SecureStorageError::BackendUnavailable(
                "security find-generic-password timed out".to_string(),
            )
        })?
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
    use protocol::{SecretKindDto, SecureStorageMetadata};
    use std::os::unix::fs::PermissionsExt;
    use std::time::SystemTime;

    fn mk_test_storage(cfg: &str, default: &str) -> MacOsKeychainStorage {
        mk_test_storage_with_command(cfg, default, PathBuf::from("security"))
    }

    fn mk_test_storage_with_command(
        cfg: &str,
        default: &str,
        security_command: PathBuf,
    ) -> MacOsKeychainStorage {
        MacOsKeychainStorage {
            user: "tester".into(),
            config_dir: PathBuf::from(cfg),
            default_config_dir: PathBuf::from(default),
            oauth_suffix: String::new(),
            security_command: Some(security_command),
            cache: Arc::new(RwLock::new(HashMap::new())),
            generation: Arc::new(AtomicU64::new(0)),
            inflight: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn shell_quote(path: &std::path::Path) -> String {
        format!("'{}'", path.to_string_lossy().replace('\'', "'\"'\"'"))
    }

    fn fake_security_lookup() -> (tempfile::TempDir, PathBuf, PathBuf, SecureStorageData) {
        let temp = tempfile::tempdir().expect("tempdir");
        let command = temp.path().join("security");
        let counter = temp.path().join("calls");
        let response = temp.path().join("response.json");
        let payload = SecureStorageData::new(
            b"cached-value".to_vec(),
            SecureStorageMetadata {
                created_at: SystemTime::UNIX_EPOCH,
                last_accessed: None,
                kind: SecretKindDto("test_secret".to_string()),
            },
        );
        std::fs::write(
            &response,
            serde_json::to_vec(&payload).expect("serialize response"),
        )
        .expect("write response");
        let script = format!(
            "#!/bin/sh\nprintf 'x\\n' >> {}\nsleep 0.05\ncat {}\n",
            shell_quote(&counter),
            shell_quote(&response),
        );
        std::fs::write(&command, script).expect("write fake security command");
        let mut permissions = std::fs::metadata(&command)
            .expect("fake command metadata")
            .permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&command, permissions).expect("chmod fake command");
        (temp, command, counter, payload)
    }

    fn fake_security_store() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().expect("tempdir");
        let command = temp.path().join("security");
        let state = temp.path().join("state");
        std::fs::create_dir(&state).expect("create fake keychain state");
        let script = format!(
            r#"#!/bin/sh
set -eu
state={}
if [ "${{1:-}}" = "-i" ]; then
  IFS= read -r line
  eval "set -- $line"
fi
operation=$1
shift
account=
service=
hex_value=
trusted_application=
while [ "$#" -gt 0 ]; do
  case "$1" in
    -a) account=$2; shift 2 ;;
    -s) service=$2; shift 2 ;;
    -X) hex_value=$2; shift 2 ;;
    -T) trusted_application=$2; shift 2 ;;
    -U|-w) shift ;;
    *) shift ;;
  esac
done
entry="$state/$account--$service"
case "$operation" in
  add-generic-password)
    [ "$trusted_application" = "/usr/bin/security" ] || exit 65
    escaped=$(printf '%s' "$hex_value" | sed 's/../\\x&/g')
    printf '%b' "$escaped" > "$entry"
    ;;
  find-generic-password)
    if [ ! -f "$entry" ]; then exit 44; fi
    cat "$entry"
    ;;
  delete-generic-password)
    if [ ! -f "$entry" ]; then exit 44; fi
    rm "$entry"
    ;;
  *) exit 64 ;;
esac
"#,
            shell_quote(&state),
        );
        std::fs::write(&command, script).expect("write fake security command");
        let mut permissions = std::fs::metadata(&command)
            .expect("fake command metadata")
            .permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&command, permissions).expect("chmod fake command");
        (temp, command)
    }

    fn invocation_count(counter: &std::path::Path) -> usize {
        std::fs::read_to_string(counter)
            .unwrap_or_default()
            .lines()
            .count()
    }

    fn assert_payload(actual: Option<SecureStorageData>, expected: &SecureStorageData) {
        let actual = actual.expect("stored payload");
        assert_eq!(actual.expose_secret_bytes(), expected.expose_secret_bytes());
        assert_eq!(actual.metadata.kind.0, expected.metadata.kind.0);
    }

    #[test]
    fn service_name_default_dir_has_no_dir_hash() {
        let s = mk_test_storage("/Users/x/.lingxi", "/Users/x/.lingxi");
        assert_eq!(
            s.keychain_service_name("-credentials"),
            "LingXi-credentials"
        );
        assert_eq!(s.keychain_service_name(""), "LingXi");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn production_backend_calls_keychain_services_without_a_cli_adapter() {
        let storage = MacOsKeychainStorage::new(
            "tester".into(),
            PathBuf::from("/Users/x/.lingxi"),
            PathBuf::from("/Users/x/.lingxi"),
            String::new(),
        )
        .expect("construct native Keychain backend");
        assert!(storage.security_command.is_none());
    }

    #[test]
    fn service_name_non_default_dir_has_dir_hash() {
        let s = mk_test_storage("/Users/x/work/.claude-2", "/Users/x/.lingxi");
        let svc = s.keychain_service_name("-credentials");
        assert!(svc.starts_with("LingXi-credentials-"));
        assert_eq!(svc.len(), "LingXi-credentials-".len() + 8);
    }

    #[test]
    fn bump_generation_increments() {
        let s = mk_test_storage("/a", "/b");
        let g0 = s.generation.load(Ordering::Acquire);
        s.bump_generation();
        let g1 = s.generation.load(Ordering::Acquire);
        assert_eq!(g1, g0 + 1);
    }

    #[tokio::test]
    async fn cache_hit_avoids_second_subprocess() {
        let (_temp, command, counter, expected) = fake_security_lookup();
        let storage = mk_test_storage_with_command("/a", "/b", command);

        let first = storage
            .retrieve("-credentials", "account")
            .await
            .expect("cold lookup");
        let second = storage
            .retrieve("-credentials", "account")
            .await
            .expect("cached lookup");

        assert_payload(first, &expected);
        assert_payload(second, &expected);
        assert_eq!(invocation_count(&counter), 1);
    }

    #[tokio::test]
    async fn concurrent_retrieves_share_one_subprocess_result() {
        let (_temp, command, counter, expected) = fake_security_lookup();
        let storage = Arc::new(mk_test_storage_with_command("/a", "/b", command));
        let mut joins = Vec::new();
        for _ in 0..20 {
            let storage = storage.clone();
            joins.push(tokio::spawn(async move {
                storage.retrieve("-credentials", "account").await
            }));
        }

        for join in joins {
            assert_payload(join.await.expect("join").expect("lookup"), &expected);
        }
        assert_eq!(invocation_count(&counter), 1);
    }

    fn test_payload(value: &[u8]) -> SecureStorageData {
        SecureStorageData::new(
            value.to_vec(),
            SecureStorageMetadata {
                created_at: SystemTime::UNIX_EPOCH,
                last_accessed: None,
                kind: SecretKindDto("test_secret".to_string()),
            },
        )
    }

    #[tokio::test]
    async fn store_and_cold_retrieve_round_trip_through_security_command() {
        let (_temp, command) = fake_security_store();
        let writer = mk_test_storage_with_command("/a", "/b", command.clone());
        let expected = test_payload(b"round-trip-value");
        writer
            .store("-credentials", "account", expected.clone())
            .await
            .expect("store");

        let reader = mk_test_storage_with_command("/a", "/b", command);
        let actual = reader
            .retrieve("-credentials", "account")
            .await
            .expect("cold retrieve");
        assert_payload(actual, &expected);
    }

    #[tokio::test]
    async fn oversized_store_argv_fallback_preserves_security_acl() {
        let (_temp, command) = fake_security_store();
        let writer = mk_test_storage_with_command("/a", "/b", command.clone());
        let expected = test_payload(&vec![b'x'; 3_000]);
        writer
            .store("-credentials", "large-account", expected.clone())
            .await
            .expect("store through argv fallback");

        let reader = mk_test_storage_with_command("/a", "/b", command);
        let actual = reader
            .retrieve("-credentials", "large-account")
            .await
            .expect("cold retrieve");
        assert_payload(actual, &expected);
    }

    #[tokio::test]
    async fn security_command_keeps_accounts_isolated() {
        let (_temp, command) = fake_security_store();
        let writer = mk_test_storage_with_command("/a", "/b", command.clone());
        let first = test_payload(b"first");
        let second = test_payload(b"second");
        writer
            .store("-credentials", "account-a", first.clone())
            .await
            .expect("store first");
        writer
            .store("-credentials", "account-b", second.clone())
            .await
            .expect("store second");

        let reader = mk_test_storage_with_command("/a", "/b", command);
        assert_payload(
            reader
                .retrieve("-credentials", "account-a")
                .await
                .expect("retrieve first"),
            &first,
        );
        assert_payload(
            reader
                .retrieve("-credentials", "account-b")
                .await
                .expect("retrieve second"),
            &second,
        );
    }

    #[tokio::test]
    async fn delete_removes_entry_from_security_command() {
        let (_temp, command) = fake_security_store();
        let storage = mk_test_storage_with_command("/a", "/b", command.clone());
        storage
            .store("-credentials", "account", test_payload(b"delete-me"))
            .await
            .expect("store");
        storage
            .delete("-credentials", "account")
            .await
            .expect("delete");

        let reader = mk_test_storage_with_command("/a", "/b", command);
        assert!(reader
            .retrieve("-credentials", "account")
            .await
            .expect("retrieve after delete")
            .is_none());
    }
}
