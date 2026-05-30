// lingxi-code/crates/core/src/settings/loader.rs
//! 4-layer source orchestration.
//!
//! Task 7 lands [`read_settings_file`] (one file at a time). Task 8 layers
//! the four sources (env > user > project > defaults) and produces the
//! final [`crate::settings::Settings`] entry point.

use crate::settings::schema::SettingsJson;
use crate::settings::SettingsError;
use std::path::{Path, PathBuf};

/// Read one settings file from disk.
///
/// # Errors
///
/// - [`SettingsError::ParseError`] on malformed JSON or unknown fields (the
///   latter via `#[serde(deny_unknown_fields)]`).
/// - [`SettingsError::Io`] on permission-denied or other non-`NotFound` I/O
///   failure. `NotFound` is NOT an error — it returns `Ok(None)`.
/// - [`SettingsError::SchemaViolation`] when [`SettingsJson::validate`] rejects
///   the parsed file.
pub fn read_settings_file(path: &Path) -> Result<Option<SettingsJson>, SettingsError> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(SettingsError::Io {
                path: path.to_path_buf(),
                source: e,
            })
        }
    };
    let parsed: SettingsJson =
        serde_json::from_slice(&bytes).map_err(|e| SettingsError::ParseError {
            path: path.to_path_buf(),
            source: e,
        })?;
    parsed.validate()?;
    Ok(Some(parsed))
}

/// Path to the user settings file: `~/.claude/settings.json`.
///
/// Uses `HOME` env var with a `dirs::home_dir`-equivalent fallback. Returns
/// `None` if neither resolves (e.g. on a misconfigured CI runner).
#[must_use]
pub fn user_settings_path() -> Option<PathBuf> {
    home_dir().map(|h| h.join(".claude").join("settings.json"))
}

/// Path to the project settings file: `<project_dir>/.claude/settings.json`.
#[must_use]
pub fn project_settings_path(project_dir: &Path) -> PathBuf {
    project_dir.join(".claude").join("settings.json")
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn returns_none_when_file_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("does_not_exist.json");
        let parsed = read_settings_file(&path).unwrap();
        assert!(parsed.is_none());
    }

    #[test]
    fn returns_settings_for_valid_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, r#"{{"trustedDirectories": ["/foo"]}}"#).unwrap();
        let parsed = read_settings_file(&path).unwrap().unwrap();
        assert_eq!(
            parsed.trusted_directories.as_deref(),
            Some(&["/foo".to_string()][..])
        );
    }

    #[test]
    fn returns_parse_error_for_malformed_json() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "{{ not valid json").unwrap();
        let err = read_settings_file(&path).unwrap_err();
        assert!(
            matches!(err, crate::settings::SettingsError::ParseError { .. }),
            "expected ParseError, got: {err:?}"
        );
    }

    #[test]
    fn returns_schema_violation_for_unknown_field() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        let mut f = std::fs::File::create(&path).unwrap();
        // deny_unknown_fields surfaces as a ParseError because serde catches it.
        writeln!(f, r#"{{"bogusField": 1}}"#).unwrap();
        let err = read_settings_file(&path).unwrap_err();
        assert!(matches!(
            err,
            crate::settings::SettingsError::ParseError { .. }
        ));
    }
}
