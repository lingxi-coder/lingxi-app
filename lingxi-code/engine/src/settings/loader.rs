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
/// - [`SettingsError::ParseError`] on malformed JSON or a known field whose
///   value has the wrong type. Unknown fields are NOT an error — they are
///   tolerated-and-ignored, matching claude-code's zod `.passthrough()`
///   schema (`types.ts:1072`; `safeParse` at `settings.ts:219`).
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
    config_home_dir().map(|h| h.join("settings.json"))
}

/// User config-home: `$branding::CONFIG_DIR_ENV` when set (`??`: an empty value
/// is honored verbatim → cwd-relative), else `<home>/<branding::DOT_DIR>`.
/// Returns `None` only when neither the env override nor `$HOME` resolves.
#[must_use]
fn config_home_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(branding::CONFIG_DIR_ENV) {
        return Some(PathBuf::from(dir));
    }
    home_dir().map(|h| branding::config_home(&h, None))
}

/// Path to the project settings file: `<project_dir>/<branding::DOT_DIR>/settings.json`.
#[must_use]
pub fn project_settings_path(project_dir: &Path) -> PathBuf {
    project_dir.join(branding::DOT_DIR).join("settings.json")
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
    fn tolerates_unknown_fields_like_zod_passthrough() {
        // claude-code's zod SettingsSchema is `.passthrough()` (types.ts:1072;
        // safeParse at settings.ts:219) — unknown keys never fail a TS load.
        // The old strictness was a parity divergence that made e.g. /effort's
        // persisted `effortLevel` silently kill the whole settings load.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"model": "opus", "effortLevel": "high", "skipDangerousModePermissionPrompt": true, "env": {"DISABLE_AUTOUPDATER": "1"}, "futureKey": [1,2]}"#,
        )
        .unwrap();
        let settings = read_settings_file(&path)
            .expect("must load")
            .expect("must be Some");
        assert_eq!(settings.model.as_deref(), Some("opus"));
    }

    #[test]
    fn wrong_typed_known_field_is_still_a_parse_error() {
        // Unknown-key tolerance must NOT loosen typed parses: a KNOWN field
        // with the wrong type still fails the load.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"model": 5}"#).unwrap();
        let err = read_settings_file(&path).unwrap_err();
        assert!(
            matches!(err, crate::settings::SettingsError::ParseError { .. }),
            "expected ParseError, got: {err:?}"
        );
    }
}
