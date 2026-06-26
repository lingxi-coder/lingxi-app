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

/// Test-only support — process-wide mutex shared by every settings test that
/// mutates `HOME` to redirect [`loader::user_settings_path`] at a tempdir.
/// All such tests must lock this before calling `std::env::set_var("HOME", ...)`
/// so the parallel test runner can't observe another test's `HOME`.
#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::Mutex;

    pub(crate) static HOME_LOCK: Mutex<()> = Mutex::new(());
}

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
    /// Semantic validation ([`SettingsJson::validate`]) rejected the file
    /// (e.g. empty string in an array field). Unknown fields are NOT a
    /// violation — they are tolerated-and-ignored (zod `.passthrough()`
    /// parity, `types.ts:1072`).
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
    /// Project root — used to locate `<project_dir>/.lingxi/settings.json`.
    pub project_dir: &'a std::path::Path,
    /// Defaults baseline. Lowest priority.
    pub defaults: SettingsJson,
}

/// Result of [`Settings::load`].
#[derive(Debug, Clone, PartialEq)]
pub struct EffectiveSettings {
    /// The merged settings.
    pub settings: SettingsJson,
    /// Per-field provenance trace for `/doctor` (M6).
    pub trace: tracer::ProvenanceTrace,
}

impl EffectiveSettings {
    /// Look up provenance for a top-level settings field.
    ///
    /// Field names are camelCase wire identifiers (e.g. `trustedDirectories`),
    /// matching the JSON keys — NOT the Rust `snake_case` field names.
    /// Returns `None` if no layer ever set this field.
    #[must_use]
    pub fn effective_for(&self, field: &str) -> Option<&tracer::FieldProvenance> {
        self.trace.by_field.get(field)
    }
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

        let mut trace = tracer::ProvenanceTrace::default();

        // Layer 1 (lowest): defaults
        trace.record_layer(tracer::Source::Defaults, &defaults);
        let mut acc = defaults;

        // Layer 2: project
        let project_path = loader::project_settings_path(project_dir);
        if let Some(proj) = loader::read_settings_file(&project_path)? {
            trace.record_layer(tracer::Source::Project, &proj);
            acc = merger::merge(acc, proj);
        }

        // Layer 3: user
        if let Some(user_path) = loader::user_settings_path() {
            if let Some(usr) = loader::read_settings_file(&user_path)? {
                trace.record_layer(tracer::Source::User, &usr);
                acc = merger::merge(acc, usr);
            }
        }

        // Layer 4 (highest): env
        let (env_layer, _invalid_env) = env_parser::parse_env(env)?;
        trace.record_layer(tracer::Source::Env, &env_layer);
        acc = merger::merge(acc, env_layer);

        Ok(EffectiveSettings {
            settings: acc,
            trace,
        })
    }

    /// Like [`Settings::load`] but GATES the user / project file layers — the
    /// substrate for claude-code's `--setting-sources <user,project,local>`
    /// (scope which setting sources load). `include_user` / `include_project`
    /// select whether the `~/.lingxi/settings.json` and
    /// `<project>/.lingxi/settings.json` layers contribute; `defaults` and the
    /// `env` layer ALWAYS apply (env vars are not a "setting source" claude
    /// scopes off). Both `true` is identical to [`Settings::load`].
    ///
    /// NOTE: lingxi has no separate "local" (`settings.local.json`) layer, so
    /// claude's `local` source is not modeled here — callers map it onto the
    /// project layer or ignore it.
    ///
    /// # Errors
    /// Same as [`Settings::load`].
    pub fn load_scoped(
        inputs: LoadInputs<'_>,
        include_user: bool,
        include_project: bool,
    ) -> Result<EffectiveSettings, SettingsError> {
        let LoadInputs {
            env,
            project_dir,
            defaults,
        } = inputs;

        let mut trace = tracer::ProvenanceTrace::default();

        // Layer 1 (lowest): defaults (always).
        trace.record_layer(tracer::Source::Defaults, &defaults);
        let mut acc = defaults;

        // Layer 2: project (gated).
        if include_project {
            let project_path = loader::project_settings_path(project_dir);
            if let Some(proj) = loader::read_settings_file(&project_path)? {
                trace.record_layer(tracer::Source::Project, &proj);
                acc = merger::merge(acc, proj);
            }
        }

        // Layer 3: user (gated).
        if include_user {
            if let Some(user_path) = loader::user_settings_path() {
                if let Some(usr) = loader::read_settings_file(&user_path)? {
                    trace.record_layer(tracer::Source::User, &usr);
                    acc = merger::merge(acc, usr);
                }
            }
        }

        // Layer 4 (highest): env (always — not a file "setting source").
        let (env_layer, _invalid_env) = env_parser::parse_env(env)?;
        trace.record_layer(tracer::Source::Env, &env_layer);
        acc = merger::merge(acc, env_layer);

        Ok(EffectiveSettings {
            settings: acc,
            trace,
        })
    }

    /// Same as [`Settings::load`] but emits telemetry through the supplied bus.
    ///
    /// Synchronous [`Settings::load`] stays available for callers that don't
    /// want async-color. This one is async because
    /// [`telemetry::AnalyticsBus::log_event`] is async.
    ///
    /// Three events fire:
    /// - `tengu_settings_loaded` on every successful load,
    /// - `tengu_settings_invalid_env` once per unrecognized env var, and
    /// - `tengu_settings_parse_error` before returning a `ParseError` /
    ///   `SchemaViolation` / `Io` from a per-file read.
    ///
    /// `bus = None` means "no telemetry sink wired" and is the path unit tests
    /// take when they don't care about events.
    ///
    /// # Errors
    ///
    /// Same as [`Settings::load`]. On a read error from a settings file,
    /// `tengu_settings_parse_error` is emitted before the error is returned.
    #[allow(clippy::too_many_lines)]
    pub async fn load_with_telemetry(
        inputs: LoadInputs<'_>,
        bus: Option<&std::sync::Arc<telemetry::AnalyticsBus>>,
    ) -> Result<EffectiveSettings, SettingsError> {
        let LoadInputs {
            env,
            project_dir,
            defaults,
        } = inputs;
        let mut trace = tracer::ProvenanceTrace::default();
        let mut layers_present: i64 = 0;
        let user_path = loader::user_settings_path();
        let mut had_env_override = false;

        // Layer 1 (lowest): defaults
        trace.record_layer(tracer::Source::Defaults, &defaults);
        let mut acc = defaults;
        layers_present += 1;

        // Layer 2: project
        let project_path = loader::project_settings_path(project_dir);
        match loader::read_settings_file(&project_path) {
            Ok(Some(proj)) => {
                trace.record_layer(tracer::Source::Project, &proj);
                acc = merger::merge(acc, proj);
                layers_present += 1;
            }
            Ok(None) => {}
            Err(e) => {
                emit_parse_error(bus, &project_path, &e).await;
                return Err(e);
            }
        }

        // Layer 3: user
        if let Some(ref up) = user_path {
            match loader::read_settings_file(up) {
                Ok(Some(usr)) => {
                    trace.record_layer(tracer::Source::User, &usr);
                    acc = merger::merge(acc, usr);
                    layers_present += 1;
                }
                Ok(None) => {}
                Err(e) => {
                    emit_parse_error(bus, up, &e).await;
                    return Err(e);
                }
            }
        }

        // Layer 4 (highest): env
        let (env_layer, invalid_env) = env_parser::parse_env(env)?;
        let env_was_nonempty = env_layer.model.is_some()
            || env_layer.telemetry_enabled.is_some()
            || env_layer.trusted_directories.is_some()
            || env_layer.additional_directories.is_some()
            || env_layer.enabled_tools.is_some()
            || env_layer.additional_includes.is_some();
        if env_was_nonempty {
            had_env_override = true;
            layers_present += 1;
        }
        trace.record_layer(tracer::Source::Env, &env_layer);
        acc = merger::merge(acc, env_layer);

        // Emit per-invalid-env event before the loaded event.
        if let Some(bus) = bus {
            for (var, value) in invalid_env {
                let mut md = telemetry::LogEventMetadata::new();
                md.insert(
                    "var".into(),
                    telemetry::AnalyticsValue::String(
                        telemetry::Verified::assert_safe(var).as_str().to_string(),
                    ),
                );
                md.insert(
                    "_PROTO_value".into(),
                    telemetry::AnalyticsValue::String(
                        telemetry::PiiTagged::assert_pii_tagged_column(value).into_inner(),
                    ),
                );
                bus.log_event("tengu_settings_invalid_env", md).await;
            }
        }

        // Emit the success event.
        if let Some(bus) = bus {
            let mut md = telemetry::LogEventMetadata::new();
            md.insert(
                "layers_present".into(),
                telemetry::AnalyticsValue::Int(layers_present),
            );
            md.insert(
                "had_env_override".into(),
                telemetry::AnalyticsValue::Bool(had_env_override),
            );
            if let Some(up) = &user_path {
                md.insert(
                    "_PROTO_user_path".into(),
                    telemetry::AnalyticsValue::String(
                        telemetry::PiiTagged::assert_pii_tagged_column(up.display().to_string())
                            .into_inner(),
                    ),
                );
            }
            bus.log_event("tengu_settings_loaded", md).await;
        }

        Ok(EffectiveSettings {
            settings: acc,
            trace,
        })
    }
}

/// Emit `tengu_settings_parse_error` on the bus before returning a settings-file
/// read error to the caller. PII is routed under `_PROTO_path`; the structural
/// error class lives in a `Verified` field — never echo the raw error message
/// because it may include file content.
async fn emit_parse_error(
    bus: Option<&std::sync::Arc<telemetry::AnalyticsBus>>,
    path: &std::path::Path,
    err: &SettingsError,
) {
    let Some(bus) = bus else { return };
    let mut md = telemetry::LogEventMetadata::new();
    md.insert(
        "_PROTO_path".into(),
        telemetry::AnalyticsValue::String(
            telemetry::PiiTagged::assert_pii_tagged_column(path.display().to_string()).into_inner(),
        ),
    );
    // Structural error class only — never the raw error message which may
    // echo file content.
    let class = match err {
        SettingsError::ParseError { .. } => "parse",
        SettingsError::SchemaViolation(_) => "schema",
        SettingsError::Io { .. } => "io",
        SettingsError::Missing(_) => "missing",
        SettingsError::InvalidEnv { .. } => "invalid_env",
    };
    md.insert(
        "error".into(),
        telemetry::AnalyticsValue::String(
            telemetry::Verified::assert_safe(class.into())
                .as_str()
                .to_string(),
        ),
    );
    bus.log_event("tengu_settings_parse_error", md).await;
}

#[cfg(test)]
mod load_tests {
    use super::*;
    use crate::settings::test_support::HOME_LOCK;
    use std::collections::BTreeMap;
    use std::io::Write;

    #[test]
    fn env_beats_user_beats_project_beats_defaults() {
        let _guard = HOME_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path();
        let user_dir = tmp.path().join("home").join(".lingxi");
        std::fs::create_dir_all(&user_dir).unwrap();
        let project_subdir = project_dir.join(".lingxi");
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
        let user_dir = tmp.path().join("home2").join(".lingxi");
        std::fs::create_dir_all(&user_dir).unwrap();
        let project_subdir = project_dir.join(".lingxi");
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
        let user_dir = tmp.path().join("home3").join(".lingxi");
        std::fs::create_dir_all(&user_dir).unwrap();
        let project_subdir = project_dir.join(".lingxi");
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

    // We deliberately hold the `HOME_LOCK` std::sync::Mutex across `.await`
    // in these tests — it serializes the whole test against other HOME
    // mutators, which is the whole point. Switching to tokio's async Mutex
    // would break the non-async load_tests sharing the same lock.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn emits_tengu_settings_loaded_with_pii_routing() {
        use async_trait::async_trait;
        use std::sync::{Arc, Mutex};
        use telemetry::{AnalyticsBus, AnalyticsSink, AnalyticsValue, LogEventMetadata};

        #[derive(Default)]
        struct CaptureSink {
            events: Mutex<Vec<(String, LogEventMetadata)>>,
        }
        #[async_trait]
        impl AnalyticsSink for CaptureSink {
            async fn log_event(&self, name: &str, m: LogEventMetadata) {
                self.events.lock().unwrap().push((name.to_string(), m));
            }
            async fn log_event_async(&self, name: &str, m: LogEventMetadata) {
                self.log_event(name, m).await;
            }
            fn name(&self) -> &str {
                "capture"
            }
        }

        let _guard = HOME_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let bus = Arc::new(AnalyticsBus::new());
        let sink: Arc<CaptureSink> = Arc::new(CaptureSink::default());
        bus.attach_sink(sink.clone() as Arc<dyn AnalyticsSink>)
            .await;

        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", tmp.path().join("home_t10"));
        let mut env = std::collections::BTreeMap::new();
        env.insert("LINGXI_MODEL".to_string(), "opus".to_string());

        let _eff = Settings::load_with_telemetry(
            LoadInputs {
                env: &env,
                project_dir: tmp.path(),
                defaults: schema::SettingsJson::default(),
            },
            Some(&bus),
        )
        .await
        .unwrap();

        let captured = sink.events.lock().unwrap().clone();
        let loaded = captured
            .iter()
            .find(|(n, _)| n == "tengu_settings_loaded")
            .expect("tengu_settings_loaded must be emitted");
        assert!(matches!(
            loaded.1.get("had_env_override"),
            Some(AnalyticsValue::Bool(true))
        ));
        assert!(
            loaded.1.contains_key("_PROTO_user_path"),
            "user path is PII; must be PROTO-routed"
        );
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn emits_tengu_settings_invalid_env_for_bad_bool() {
        use async_trait::async_trait;
        use std::sync::{Arc, Mutex};
        use telemetry::{AnalyticsBus, AnalyticsSink, AnalyticsValue, LogEventMetadata};

        #[derive(Default)]
        struct CaptureSink {
            events: Mutex<Vec<(String, LogEventMetadata)>>,
        }
        #[async_trait]
        impl AnalyticsSink for CaptureSink {
            async fn log_event(&self, n: &str, m: LogEventMetadata) {
                self.events.lock().unwrap().push((n.to_string(), m));
            }
            async fn log_event_async(&self, n: &str, m: LogEventMetadata) {
                self.log_event(n, m).await;
            }
            fn name(&self) -> &str {
                "capture"
            }
        }

        let _guard = HOME_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let bus = Arc::new(AnalyticsBus::new());
        let sink: Arc<CaptureSink> = Arc::new(CaptureSink::default());
        bus.attach_sink(sink.clone() as Arc<dyn AnalyticsSink>)
            .await;

        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", tmp.path().join("home_t10b"));
        let mut env = std::collections::BTreeMap::new();
        env.insert("LINGXI_TELEMETRY_ENABLED".to_string(), "yes".to_string());

        let _ = Settings::load_with_telemetry(
            LoadInputs {
                env: &env,
                project_dir: tmp.path(),
                defaults: schema::SettingsJson::default(),
            },
            Some(&bus),
        )
        .await
        .unwrap();

        let captured = sink.events.lock().unwrap().clone();
        let invalid = captured
            .iter()
            .find(|(n, _)| n == "tengu_settings_invalid_env")
            .expect("tengu_settings_invalid_env must be emitted");
        assert!(matches!(
            invalid.1.get("var"),
            Some(AnalyticsValue::String(s)) if s == "LINGXI_TELEMETRY_ENABLED"
        ));
        assert!(invalid.1.contains_key("_PROTO_value"));
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn emits_tengu_settings_parse_error_on_malformed_project_json() {
        use async_trait::async_trait;
        use std::sync::{Arc, Mutex};
        use telemetry::{AnalyticsBus, AnalyticsSink, AnalyticsValue, LogEventMetadata};

        #[derive(Default)]
        struct CaptureSink {
            events: Mutex<Vec<(String, LogEventMetadata)>>,
        }
        #[async_trait]
        impl AnalyticsSink for CaptureSink {
            async fn log_event(&self, n: &str, m: LogEventMetadata) {
                self.events.lock().unwrap().push((n.to_string(), m));
            }
            async fn log_event_async(&self, n: &str, m: LogEventMetadata) {
                self.log_event(n, m).await;
            }
            fn name(&self) -> &str {
                "capture"
            }
        }

        let _guard = HOME_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let bus = Arc::new(AnalyticsBus::new());
        let sink: Arc<CaptureSink> = Arc::new(CaptureSink::default());
        bus.attach_sink(sink.clone() as Arc<dyn AnalyticsSink>)
            .await;

        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", tmp.path().join("home_t10c"));
        let project_subdir = tmp.path().join(".lingxi");
        std::fs::create_dir_all(&project_subdir).unwrap();
        // Malformed JSON in the project layer.
        let mut pf = std::fs::File::create(project_subdir.join("settings.json")).unwrap();
        writeln!(pf, "{{not-json").unwrap();

        let result = Settings::load_with_telemetry(
            LoadInputs {
                env: &BTreeMap::new(),
                project_dir: tmp.path(),
                defaults: schema::SettingsJson::default(),
            },
            Some(&bus),
        )
        .await;
        assert!(result.is_err(), "malformed JSON must surface as Err");

        let captured = sink.events.lock().unwrap().clone();
        let parse_err = captured
            .iter()
            .find(|(n, _)| n == "tengu_settings_parse_error")
            .expect("tengu_settings_parse_error must be emitted before the error returns");
        assert!(parse_err.1.contains_key("_PROTO_path"));
        assert!(matches!(
            parse_err.1.get("error"),
            Some(AnalyticsValue::String(s)) if s == "parse"
        ));
    }
}
