//! `--cwd` handling: validate + chdir before orchestrator initialisation.
//!
//! Mutates global process state via `std::env::set_current_dir`; callers
//! invoke this once at startup BEFORE any background tokio task spawns.

use std::path::{Path, PathBuf};

/// Errors surfaced by [`apply_cwd`].
#[derive(Debug, thiserror::Error)]
pub enum CwdError {
    /// The supplied path does not exist.
    #[error("--cwd path does not exist: {0}")]
    NotFound(PathBuf),
    /// The supplied path exists but is not a directory.
    #[error("--cwd path is not a directory: {0}")]
    NotADirectory(PathBuf),
    /// `std::env::set_current_dir` failed.
    #[error("could not change directory to {0}: {1}")]
    ChdirFailed(PathBuf, std::io::Error),
}

/// Apply `--cwd` if set: validate + `std::env::set_current_dir`.
pub fn apply_cwd(cwd: Option<&Path>) -> Result<(), CwdError> {
    let Some(p) = cwd else {
        return Ok(());
    };
    if !p.exists() {
        return Err(CwdError::NotFound(p.to_path_buf()));
    }
    if !p.is_dir() {
        return Err(CwdError::NotADirectory(p.to_path_buf()));
    }
    std::env::set_current_dir(p).map_err(|e| CwdError::ChdirFailed(p.to_path_buf(), e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_is_ok() {
        assert!(apply_cwd(None).is_ok());
    }

    #[test]
    fn nonexistent_path_errors() {
        let r = apply_cwd(Some(Path::new("/this/path/does/not/exist/at/all")));
        assert!(matches!(r, Err(CwdError::NotFound(_))));
    }

    #[test]
    fn file_path_errors_as_not_a_directory() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let r = apply_cwd(Some(tmp.path()));
        assert!(matches!(r, Err(CwdError::NotADirectory(_))));
    }

    // Note: a "successful chdir" test would mutate process-wide CWD and
    // race with other tests in the same binary that read `current_dir`.
    // Validation-only tests above already cover the happy-path branch
    // shape; the chdir call itself is a single `std::env::set_current_dir`
    // line whose behavior is the std library's responsibility.
}
