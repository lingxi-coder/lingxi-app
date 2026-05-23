//! M3-01 — 4-layer settings loader (env > user > project > defaults).
//!
//! Entry point: [`Settings::load`]. Per-field merge rules live in
//! [`merger`]; provenance for `/doctor` (M6) lives in [`tracer`].
//!
//! See spec §4 Flow D for the full data flow.

use std::path::PathBuf;

pub mod env_parser;
pub mod loader;
pub mod merger;
pub mod schema;
pub mod tracer;

/// Errors returned by [`Settings::load`] and its sub-modules.
///
/// Mirrors spec §5 `SettingsError`.
#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    /// A settings file at this path could not be found.
    #[error("settings file not found: {0}")]
    Missing(PathBuf),
    /// JSON parse failure for the file at this path.
    #[error("settings file malformed at {path}: {source}")]
    ParseError {
        /// The file whose JSON failed to parse.
        path: PathBuf,
        /// The underlying serde error.
        #[source]
        source: serde_json::Error,
    },
    /// An env var holds a value that can't be coerced into its target type.
    #[error("env var {var} has invalid value {value:?}")]
    InvalidEnv {
        /// The env var name (e.g. `LINGXI_TRUSTED_DIRECTORIES`).
        var: String,
        /// The raw value as received from the process env.
        value: String,
    },
    /// Schema validation rejected the file (unknown field, type mismatch).
    #[error("schema validation failed: {0}")]
    SchemaViolation(String),
    /// Underlying IO failure (permission denied, etc.).
    #[error("io error reading {path}: {source}")]
    Io {
        /// The file the I/O happened on.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
}

pub use schema::SettingsJson;

/// Inputs to [`Settings::load`].
///
/// We pass env in explicitly rather than calling `std::env::vars()` so unit
/// tests can construct a deterministic snapshot.
#[derive(Debug)]
pub struct LoadInputs<'a> {
    /// Process env snapshot.
    pub env: &'a std::collections::BTreeMap<String, String>,
    /// Project root — used to locate `<project_dir>/.claude/settings.json`.
    pub project_dir: &'a std::path::Path,
    /// Defaults baseline. Lowest priority.
    pub defaults: SettingsJson,
}

/// Result of [`Settings::load`].
#[derive(Debug, Clone)]
pub struct EffectiveSettings {
    /// The merged settings.
    pub settings: SettingsJson,
    /// Per-field provenance trace for `/doctor` (M6). Wired in Task 9.
    pub trace: tracer::ProvenanceTrace,
}

/// Public entry point. See spec §4 Flow D for the data-flow diagram.
#[non_exhaustive]
pub struct Settings;

impl Settings {
    /// Load the 4-layer merged settings.
    ///
    /// Priority (highest first): env → user → project → defaults.
    /// Merge order (call order in code): defaults → project → user → env,
    /// because [`merger::merge`] is `(prev, next)` where `next` overrides.
    ///
    /// # Errors
    ///
    /// Any [`SettingsError`] from a sub-layer bubbles up. Missing files at
    /// the user or project layers are NOT errors — they just contribute
    /// an empty layer.
    pub fn load(inputs: LoadInputs<'_>) -> Result<EffectiveSettings, SettingsError> {
        let LoadInputs {
            env,
            project_dir,
            defaults,
        } = inputs;

        // Layer 1 (lowest): defaults
        let mut acc = defaults;

        // Layer 2: project
        let project_path = loader::project_settings_path(project_dir);
        if let Some(proj) = loader::read_settings_file(&project_path)? {
            acc = merger::merge(acc, proj);
        }

        // Layer 3: user
        if let Some(user_path) = loader::user_settings_path() {
            if let Some(usr) = loader::read_settings_file(&user_path)? {
                acc = merger::merge(acc, usr);
            }
        }

        // Layer 4 (highest): env
        let (env_layer, _invalid_env) = env_parser::parse_env(env)?;
        acc = merger::merge(acc, env_layer);

        Ok(EffectiveSettings {
            settings: acc,
            trace: tracer::ProvenanceTrace::default(),
        })
    }
}

#[cfg(test)]
mod load_tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::io::Write;
    use std::sync::Mutex;

    /// `Settings::load` reads `HOME` via [`loader::user_settings_path`], and
    /// these tests mutate `HOME` to point at a tempdir. Mutex serializes them
    /// so the parallel test runner can't observe another test's `HOME`.
    static HOME_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn env_beats_user_beats_project_beats_defaults() {
        let _guard = HOME_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path();
        let user_dir = tmp.path().join("home").join(".claude");
        std::fs::create_dir_all(&user_dir).unwrap();
        let project_subdir = project_dir.join(".claude");
        std::fs::create_dir_all(&project_subdir).unwrap();

        let mut pf = std::fs::File::create(project_subdir.join("settings.json")).unwrap();
        writeln!(pf, r#"{{"model": "project-model"}}"#).unwrap();

        let mut uf = std::fs::File::create(user_dir.join("settings.json")).unwrap();
        writeln!(uf, r#"{{"model": "user-model"}}"#).unwrap();

        // Stand in for $HOME so user_settings_path() points to our tempdir.
        std::env::set_var("HOME", tmp.path().join("home"));

        let mut env = BTreeMap::new();
        env.insert("LINGXI_MODEL".to_string(), "env-model".to_string());

        let defaults = schema::SettingsJson {
            model: Some("default-model".to_string()),
            ..Default::default()
        };

        let eff = Settings::load(LoadInputs {
            env: &env,
            project_dir,
            defaults,
        })
        .unwrap();
        assert_eq!(eff.settings.model.as_deref(), Some("env-model"));
    }

    #[test]
    fn user_beats_project_when_no_env() {
        let _guard = HOME_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path();
        let user_dir = tmp.path().join("home2").join(".claude");
        std::fs::create_dir_all(&user_dir).unwrap();
        let project_subdir = project_dir.join(".claude");
        std::fs::create_dir_all(&project_subdir).unwrap();

        let mut pf = std::fs::File::create(project_subdir.join("settings.json")).unwrap();
        writeln!(pf, r#"{{"model": "project-model"}}"#).unwrap();
        let mut uf = std::fs::File::create(user_dir.join("settings.json")).unwrap();
        writeln!(uf, r#"{{"model": "user-model"}}"#).unwrap();

        std::env::set_var("HOME", tmp.path().join("home2"));

        let eff = Settings::load(LoadInputs {
            env: &BTreeMap::new(),
            project_dir,
            defaults: schema::SettingsJson::default(),
        })
        .unwrap();
        assert_eq!(eff.settings.model.as_deref(), Some("user-model"));
    }

    #[test]
    fn array_concat_dedup_runs_through_4_layers() {
        let _guard = HOME_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path();
        let user_dir = tmp.path().join("home3").join(".claude");
        std::fs::create_dir_all(&user_dir).unwrap();
        let project_subdir = project_dir.join(".claude");
        std::fs::create_dir_all(&project_subdir).unwrap();

        let mut pf = std::fs::File::create(project_subdir.join("settings.json")).unwrap();
        writeln!(pf, r#"{{"trustedDirectories": ["/project"]}}"#).unwrap();
        let mut uf = std::fs::File::create(user_dir.join("settings.json")).unwrap();
        writeln!(uf, r#"{{"trustedDirectories": ["/user"]}}"#).unwrap();
        std::env::set_var("HOME", tmp.path().join("home3"));

        let mut env = BTreeMap::new();
        env.insert("LINGXI_TRUSTED_DIRECTORIES".to_string(), "/env".to_string());

        let defaults = schema::SettingsJson {
            trusted_directories: Some(vec!["/default".into()]),
            ..Default::default()
        };

        let eff = Settings::load(LoadInputs {
            env: &env,
            project_dir,
            defaults,
        })
        .unwrap();
        assert_eq!(
            eff.settings.trusted_directories.as_deref(),
            Some(
                &[
                    "/default".to_string(),
                    "/project".to_string(),
                    "/user".to_string(),
                    "/env".to_string()
                ][..]
            ),
            "all four layers contribute in low-to-high priority order"
        );
    }
}
