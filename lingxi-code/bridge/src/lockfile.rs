//! `~/.lingxi/ide/<port>.lock` writer and Drop-guard.
//!
//! Wire format matches claude-code's `LockfileJsonContent` in
//! `src/utils/ide.ts`. The file is JSON with keys:
//! `pid`, `workspaceFolders`, `ideName`, `transport`, `runningInWindows`,
//! `authToken`. The port is encoded in the filename (`<port>.lock`),
//! NOT in the JSON body.

use rand::rngs::OsRng;
use rand::TryRngCore;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The literal `ideName` value we publish in the `~/.lingxi/ide/` lockfile,
/// scanned by the real IDE peer (claude-code, VS Code Claude, …).
pub const IDE_NAME: &str = "LingXi";
/// The literal `ideName` value we publish in the dedicated `~/.lingxi/bridge/`
/// discovery lockfile. Deliberately DISTINCT from [`IDE_NAME`] so the real IDE
/// peer's `~/.lingxi/ide/` scan never picks up the bridge's lockfile.
pub const BRIDGE_IDE_NAME: &str = "LingXi-Bridge";
/// The literal `transport` value we publish — always `"ws"` for this bridge.
pub const TRANSPORT: &str = "ws";

/// JSON body of a lockfile (matches claude-code's `LockfileJsonContent`).
///
/// Field names are camelCase to match the wire format byte-for-byte.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockfileBody {
    /// Process ID of the bridge that wrote the file.
    pub pid: u32,
    /// Absolute paths of workspace folders this session owns.
    #[serde(rename = "workspaceFolders")]
    pub workspace_folders: Vec<PathBuf>,
    /// Human-readable IDE name displayed in the picker.
    #[serde(rename = "ideName")]
    pub ide_name: String,
    /// Transport selector — `"ws"` (this crate) or `"sse"` (older IDEs).
    pub transport: String,
    /// True when this bridge is hosted on Windows.
    #[serde(rename = "runningInWindows")]
    pub running_in_windows: bool,
    /// 32-char lowercase hex token a client MUST present in the
    /// `X-LingXi-Ide-Authorization` header.
    #[serde(rename = "authToken")]
    pub auth_token: String,
}

/// Self-describing lockfile: knows its directory, its port, and its JSON body.
#[derive(Debug, Clone)]
pub struct IdeLockfile {
    ide_dir: PathBuf,
    port: u16,
    body: LockfileBody,
}

impl IdeLockfile {
    /// Generate a 32-char lowercase hex auth token from `OsRng`.
    #[must_use]
    pub fn generate_auth_token() -> String {
        let mut bytes = [0u8; 16];
        // `OsRng::try_fill_bytes` returns Result in rand 0.9; unwrap is fine for
        // an OS-level source on healthy systems and the alternative is to abort.
        OsRng
            .try_fill_bytes(&mut bytes)
            .expect("OsRng must fill bytes");
        let mut out = String::with_capacity(32);
        for b in bytes {
            use std::fmt::Write;
            write!(&mut out, "{b:02x}").expect("write to String is infallible");
        }
        out
    }

    /// Build a lockfile body with a fresh auth token and the IDE-peer
    /// [`IDE_NAME`].
    #[must_use]
    pub fn new_body(workspace_folders: Vec<PathBuf>) -> LockfileBody {
        Self::new_body_with_ide_name(IDE_NAME, workspace_folders)
    }

    /// Build a lockfile body with a fresh auth token and an explicit `ide_name`
    /// (e.g. [`BRIDGE_IDE_NAME`] for the dedicated bridge discovery file).
    #[must_use]
    pub fn new_body_with_ide_name(
        ide_name: &str,
        workspace_folders: Vec<PathBuf>,
    ) -> LockfileBody {
        LockfileBody {
            pid: std::process::id(),
            workspace_folders,
            ide_name: ide_name.to_string(),
            transport: TRANSPORT.to_string(),
            running_in_windows: cfg!(target_os = "windows"),
            auth_token: Self::generate_auth_token(),
        }
    }

    /// Construct an `IdeLockfile` rooted at the user's `~/.lingxi/ide` dir.
    /// Creates the dir on first call (mode 0o755 on Unix).
    ///
    /// # Errors
    /// Returns I/O errors if the home directory cannot be resolved or the
    /// `~/.lingxi/ide` directory cannot be created.
    pub fn for_user(port: u16, workspace_folders: Vec<PathBuf>) -> std::io::Result<Self> {
        // Honor `$LINGXI_CONFIG_DIR` (set+non-empty) else `~/.lingxi`, like the
        // rest of the config-home tree, so the IDE discovery lockfile lands where
        // the engine/peers look for it.
        let config_home = std::env::var_os(branding::CONFIG_DIR_ENV)
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|h| h.join(branding::DOT_DIR)))
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "no home directory")
            })?;
        let ide_dir = config_home.join("ide");
        std::fs::create_dir_all(&ide_dir)?;
        Ok(Self::new_for_ide_dir(ide_dir, port, workspace_folders))
    }

    /// Construct an `IdeLockfile` rooted at an arbitrary `ide_dir`. Used by tests
    /// with `tempfile::TempDir` instead of `$HOME`.
    #[must_use]
    pub fn new_for_ide_dir(ide_dir: PathBuf, port: u16, workspace_folders: Vec<PathBuf>) -> Self {
        Self {
            ide_dir,
            port,
            body: Self::new_body(workspace_folders),
        }
    }

    /// Construct a DEDICATED bridge discovery lockfile rooted at the user's
    /// `~/.lingxi/bridge` dir, carrying the distinct [`BRIDGE_IDE_NAME`].
    /// Creates the dir on first call (mode 0o755 on Unix).
    ///
    /// This is the F2-04 discovery file the Electron app reads: it lives in its
    /// own directory with its own `ideName` so it never collides with the real
    /// IDE peer scanning `~/.lingxi/ide/`.
    ///
    /// # Errors
    /// Returns I/O errors if the home directory cannot be resolved or the
    /// `~/.lingxi/bridge` directory cannot be created.
    pub fn for_bridge(port: u16, workspace_folders: Vec<PathBuf>) -> std::io::Result<Self> {
        // Honor `$LINGXI_CONFIG_DIR` (set+non-empty) else `~/.lingxi`, matching
        // the config-home tree (the Electron app reads this discovery file).
        let config_home = std::env::var_os(branding::CONFIG_DIR_ENV)
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|h| h.join(branding::DOT_DIR)))
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "no home directory")
            })?;
        let bridge_dir = config_home.join("bridge");
        std::fs::create_dir_all(&bridge_dir)?;
        Ok(Self::new_for_bridge_dir(bridge_dir, port, workspace_folders))
    }

    /// Construct a bridge discovery lockfile rooted at an arbitrary
    /// `bridge_dir`, carrying the distinct [`BRIDGE_IDE_NAME`]. Used by tests
    /// with `tempfile::TempDir` instead of `$HOME`.
    #[must_use]
    pub fn new_for_bridge_dir(
        bridge_dir: PathBuf,
        port: u16,
        workspace_folders: Vec<PathBuf>,
    ) -> Self {
        Self {
            ide_dir: bridge_dir,
            port,
            body: Self::new_body_with_ide_name(BRIDGE_IDE_NAME, workspace_folders),
        }
    }

    /// Full path of this lockfile.
    #[must_use]
    pub fn path(&self) -> PathBuf {
        self.ide_dir.join(format!("{}.lock", self.port))
    }

    /// Borrow the JSON body (useful for tests).
    #[must_use]
    pub fn body(&self) -> &LockfileBody {
        &self.body
    }

    /// Borrow the auth token.
    #[must_use]
    pub fn auth_token(&self) -> &str {
        &self.body.auth_token
    }

    /// Port encoded in the filename.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Atomically write the JSON body to disk.
    ///
    /// # Errors
    /// Returns I/O errors if serialization, the temp-file write, or the
    /// rename-into-place step fails.
    pub fn write(&self) -> std::io::Result<()> {
        let serialized = serde_json::to_vec_pretty(&self.body)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        // Atomic write: tempfile in same dir + rename.
        let tmp = self.ide_dir.join(format!(".{}.lock.tmp", self.port));
        std::fs::write(&tmp, &serialized)?;
        std::fs::rename(&tmp, self.path())?;
        Ok(())
    }

    /// Parse a lockfile from disk. Returns the body plus the port encoded in
    /// the filename.
    ///
    /// # Errors
    /// Returns `InvalidData` if the file is not well-formed JSON matching
    /// [`LockfileBody`], `InvalidInput` if the filename isn't `<port>.lock`,
    /// or the underlying I/O error from reading the file.
    pub fn read(path: &Path) -> std::io::Result<(LockfileBody, u16)> {
        let raw = std::fs::read_to_string(path)?;
        let body: LockfileBody = serde_json::from_str(&raw)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let port = path
            .file_name()
            .and_then(|s| s.to_str())
            .and_then(|s| s.strip_suffix(".lock"))
            .and_then(|s| s.parse::<u16>().ok())
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "lockfile name not <port>.lock",
                )
            })?;
        Ok((body, port))
    }
}

/// Discover the most-recently-modified `<port>.lock` file in `ide_dir`.
///
/// Returns `None` if the directory does not exist, contains no `.lock` files,
/// or every candidate fails to parse. Used at bridge connect time to find a
/// running IDE peer (mirrors claude-code's `cleanupStaleIdeLockfiles` scan).
#[must_use]
pub fn discover_latest(ide_dir: &Path) -> Option<IdeLockfile> {
    let entries = std::fs::read_dir(ide_dir).ok()?;
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        // Filter to <port>.lock; reject our own `.<port>.lock.tmp` shadow.
        if path.extension().and_then(|s| s.to_str()) != Some("lock") {
            continue;
        }
        if path
            .file_name()
            .and_then(|s| s.to_str())
            .is_none_or(|s| s.starts_with('.'))
        {
            continue;
        }
        let mtime = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        match &best {
            Some((existing, _)) if *existing >= mtime => {}
            _ => best = Some((mtime, path)),
        }
    }
    let (_, path) = best?;
    let (body, port) = IdeLockfile::read(&path).ok()?;
    Some(IdeLockfile {
        ide_dir: ide_dir.to_path_buf(),
        port,
        body,
    })
}

/// Drop-guard that removes a lockfile when the bridge shuts down OR panics.
///
/// `Drop::drop` MUST be infallible-on-failure: we use `std::fs::remove_file`
/// directly (sync) because the async runtime may already be torn down during
/// panic unwind. Removal errors are logged at `warn` level and swallowed —
/// failing to remove a stale lockfile is recoverable (claude-code's
/// `cleanupStaleIdeLockfiles()` will reap it next start).
pub struct LockfileGuard {
    path: Option<PathBuf>,
}

impl LockfileGuard {
    /// Take ownership of `path`. When this guard drops, the file is removed.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    /// Surrender the guard without removing the file. Used in tests where we
    /// want to inspect the lockfile AFTER drop normally would have run.
    pub fn disarm(mut self) {
        self.path = None;
    }
}

impl Drop for LockfileGuard {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    tracing::warn!(?path, error = %e, "failed to remove lockfile in Drop");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn read_recovers_port_from_filename() {
        let tmp = TempDir::new().unwrap();
        let lf = IdeLockfile::new_for_ide_dir(
            tmp.path().to_path_buf(),
            54321,
            vec![PathBuf::from("/w")],
        );
        lf.write().unwrap();
        let (body, port) = IdeLockfile::read(&lf.path()).unwrap();
        assert_eq!(port, 54321);
        assert_eq!(body.transport, "ws");
        assert_eq!(body.ide_name, "LingXi");
    }

    #[test]
    fn read_rejects_filename_without_dot_lock() {
        let tmp = TempDir::new().unwrap();
        let bogus = tmp.path().join("40729.json");
        std::fs::write(
            &bogus,
            r#"{"pid":1,"workspaceFolders":[],"ideName":"x","transport":"ws","runningInWindows":false,"authToken":"a"}"#,
        )
        .unwrap();
        let err = IdeLockfile::read(&bogus).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn discover_latest_returns_none_for_empty_dir() {
        let tmp = TempDir::new().unwrap();
        assert!(discover_latest(tmp.path()).is_none());
    }

    #[test]
    fn discover_latest_picks_most_recent() {
        let tmp = TempDir::new().unwrap();
        let older = IdeLockfile::new_for_ide_dir(
            tmp.path().to_path_buf(),
            40001,
            vec![PathBuf::from("/w/a")],
        );
        older.write().unwrap();
        // Force a later mtime on the second file.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let newer = IdeLockfile::new_for_ide_dir(
            tmp.path().to_path_buf(),
            40002,
            vec![PathBuf::from("/w/b")],
        );
        newer.write().unwrap();
        let found = discover_latest(tmp.path()).expect("should discover");
        assert_eq!(found.port(), 40002);
    }

    #[test]
    fn discover_latest_ignores_temp_files() {
        let tmp = TempDir::new().unwrap();
        // Drop a stray `.tmp` that shouldn't be picked up.
        std::fs::write(tmp.path().join(".40729.lock.tmp"), b"{}").unwrap();
        assert!(discover_latest(tmp.path()).is_none());
    }

    #[test]
    fn disarm_skips_drop_removal() {
        let tmp = TempDir::new().unwrap();
        let lf = IdeLockfile::new_for_ide_dir(
            tmp.path().to_path_buf(),
            40729,
            vec![PathBuf::from("/w")],
        );
        lf.write().unwrap();
        let guard = LockfileGuard::new(lf.path());
        guard.disarm();
        assert!(lf.path().exists(), "disarm must preserve the file");
    }
}
