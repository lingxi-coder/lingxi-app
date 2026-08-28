//! Settings snapshot builder.
//!
//! Lives in `bridge-server` rather than `engine::settings` because the
//! `engine` crate's dependencies are deliberately minimal (no `traits`, no
//! `permission`), while `bridge-server` already depends on client-protocol,
//! permission, migrations, traits and engine. A later task adds a writer here
//! needing `permission::mark_internal_write`.
//!
//! This module only reads and merges the layered settings files into one
//! snapshot: the effective (merged) values, plus which layer each value
//! actually came from. It does not write — that is a later task.

use std::collections::BTreeMap;
use std::path::PathBuf;

use migrations::settings_update::{read_settings_map, settings_path, SettingsSource};
use serde_json::Value;

/// A layer a settings value can come from. Ordered lowest priority first,
/// matching the engine's documented merge precedence (highest first):
/// `env → managed → cli → local → project → user → defaults`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsLayer {
    /// Built-in defaults, no file backs this layer.
    Defaults,
    /// `<lingxi_home>/settings.json`.
    User,
    /// `<project_dir>/<DOT_DIR>/settings.json`.
    Project,
    /// `<project_dir>/<DOT_DIR>/settings.local.json`.
    Local,
    /// Flags passed on the command line for this run.
    Cli,
    /// Administrator-managed settings; keys here can also be locked.
    Managed,
    /// Environment variables.
    Env,
}

/// One settings file's on-disk state, as surfaced to the UI.
pub struct SettingsFile {
    /// Which layer this file backs.
    pub layer: SettingsLayer,
    /// The resolved on-disk path, regardless of whether it exists.
    pub path: PathBuf,
    /// Whether the file exists on disk.
    pub exists: bool,
    /// Whether the file parsed cleanly and can be written to (`false` when
    /// `parse_error` is set).
    pub writable: bool,
    /// The parse error, if the file exists but contains invalid JSON.
    pub parse_error: Option<String>,
}

/// A merged view of the layered settings: the effective values, which layer
/// each one actually came from, the session's active (already-loaded)
/// values, and which keys an administrator has locked via the managed layer.
pub struct SettingsSnapshot {
    /// The on-disk state of every file layer this module reads.
    pub files: Vec<SettingsFile>,
    /// The merged values: for each key, the value from its highest-priority
    /// contributing layer.
    pub effective: BTreeMap<String, Value>,
    /// The values the running session actually loaded at startup, as
    /// supplied by the caller.
    pub active: BTreeMap<String, Value>,
    /// For each key in `effective`, which layer it came from.
    pub provenance: BTreeMap<String, SettingsLayer>,
    /// Keys an administrator pinned through the managed-settings layer.
    pub locked: Vec<String>,
}

/// The two roots needed to resolve every settings layer's file path.
pub struct SettingsPaths {
    /// The user's LingXi home directory (holds the `User` layer file).
    pub lingxi_home: PathBuf,
    /// The current project's root directory (holds the `Project` and
    /// `Local` layer files, under `<project_dir>/<DOT_DIR>/`).
    pub project_dir: PathBuf,
}

/// The FILE layers this module reads and merges, ordered lowest-priority
/// first so a later entry overwrites an earlier one. Per the engine's merge
/// precedence, among these three: local beats project beats user. (`cli`,
/// `managed`, `env` and `defaults` are not file layers this module reads.)
const FILE_LAYERS: [(SettingsSource, SettingsLayer); 3] = [
    (SettingsSource::User, SettingsLayer::User),
    (SettingsSource::Project, SettingsLayer::Project),
    (SettingsSource::Local, SettingsLayer::Local),
];

/// Build a settings snapshot by reading and merging the three writable file
/// layers. `active` is the set of values the running session actually
/// loaded at startup; `locked` names the keys an administrator pinned
/// through the managed-settings layer (supplied by a later task's
/// composition root).
pub fn build_snapshot(
    paths: &SettingsPaths,
    active: BTreeMap<String, Value>,
    locked: Vec<String>,
) -> SettingsSnapshot {
    let mut effective: BTreeMap<String, Value> = BTreeMap::new();
    let mut provenance: BTreeMap<String, SettingsLayer> = BTreeMap::new();
    let mut files: Vec<SettingsFile> = Vec::new();

    for (source, layer) in FILE_LAYERS {
        let path = settings_path(source, &paths.lingxi_home, &paths.project_dir);
        let exists = path.exists();
        let (map, parse_error) = match read_settings_map(&path) {
            Ok(m) => (m, None),
            Err(e) => (serde_json::Map::new(), Some(e)),
        };
        for (key, value) in &map {
            effective.insert(key.clone(), value.clone());
            provenance.insert(key.clone(), layer);
        }
        files.push(SettingsFile {
            layer,
            path,
            exists,
            writable: parse_error.is_none(),
            parse_error,
        });
    }

    SettingsSnapshot {
        files,
        effective,
        active,
        provenance,
        locked,
    }
}

/// Resolve the file path for a **writable** destination layer. Only
/// `User` / `Project` / `Local` are writable; everything else is rejected
/// with an error naming both the rejected value and the writable set.
pub fn writable_path(paths: &SettingsPaths, layer: SettingsLayer) -> Result<PathBuf, String> {
    let source = match layer {
        SettingsLayer::User => SettingsSource::User,
        SettingsLayer::Project => SettingsSource::Project,
        SettingsLayer::Local => SettingsSource::Local,
        other => {
            return Err(format!(
                "{other:?} is not a writable settings destination; writable destinations are User, Project, Local"
            ))
        }
    };
    Ok(settings_path(source, &paths.lingxi_home, &paths.project_dir))
}

impl SettingsLayer {
    /// The layer's name as it appears in the wire payloads
    /// (`provenance_json`'s values and `files_json`'s `layer` field).
    /// Spelled out rather than derived from `Debug` so a rename of the Rust
    /// variant cannot silently change the wire contract.
    #[must_use]
    pub fn wire_name(self) -> &'static str {
        match self {
            SettingsLayer::Defaults => "defaults",
            SettingsLayer::User => "user",
            SettingsLayer::Project => "project",
            SettingsLayer::Local => "local",
            SettingsLayer::Cli => "cli",
            SettingsLayer::Managed => "managed",
            SettingsLayer::Env => "env",
        }
    }
}

/// Everything the router needs to answer the `Settings` listing, supplied once
/// by the composition root.
///
/// `active` and `locked` are inputs rather than something this crate computes:
/// `active` is what the running session actually loaded at startup, and
/// `locked` comes from the managed-settings layers the desktop composition root
/// already loads (`engine_desktop::managed_locked_setting_keys`). bridge-server
/// deliberately does not re-implement managed-layer loading.
pub struct SettingsContext {
    /// The two roots every file layer's path is resolved from.
    pub paths: SettingsPaths,
    /// The values the running session loaded at startup.
    pub active: BTreeMap<String, Value>,
    /// Keys an administrator pinned through the managed layer.
    pub locked: Vec<String>,
}

/// A [`SettingsSnapshot`] lowered to the wire's payloads.
///
/// Every structured field is a JSON **String**: `serde_json::Value` must not
/// enter `client-protocol` (decision §0.4 — it is not UniFFI-representable),
/// so structured payloads travel as strings exactly the way
/// `ToolUseStarted.input_json` does. `locked` stays a plain string list, which
/// IS representable.
pub struct LoweredSettings {
    /// `{key: value}` — the merged effective settings.
    pub effective_json: String,
    /// `{key: layer}` — which layer each effective value came from.
    pub provenance_json: String,
    /// `[{layer, path, exists, writable, parse_error?}]` — one entry per file
    /// layer, in the module's lowest-priority-first order.
    pub files_json: String,
    /// `{key: value}` — the session's actually loaded values.
    pub active_json: String,
    /// Administrator-locked keys, passed through unchanged.
    pub locked: Vec<String>,
}

/// Lower a snapshot to the wire payloads. A pure function called by the
/// router, matching how the router lowers every other reply through the pure
/// `client_adapter::lowering` fns rather than lowering inside the builder.
///
/// Serialization of a `BTreeMap<String, Value>` and of the file list cannot
/// fail (both are plain JSON objects/arrays with string keys), but a panic at
/// the routing seam is never acceptable, so a failure degrades to an empty
/// JSON document rather than unwrapping.
#[must_use]
pub fn lower_snapshot(snapshot: &SettingsSnapshot) -> LoweredSettings {
    let provenance: BTreeMap<&str, &str> = snapshot
        .provenance
        .iter()
        .map(|(key, layer)| (key.as_str(), layer.wire_name()))
        .collect();
    let files: Vec<Value> = snapshot
        .files
        .iter()
        .map(|file| {
            let mut entry = serde_json::Map::new();
            entry.insert("layer".to_string(), Value::from(file.layer.wire_name()));
            entry.insert(
                "path".to_string(),
                Value::from(file.path.to_string_lossy().into_owned()),
            );
            entry.insert("exists".to_string(), Value::from(file.exists));
            entry.insert("writable".to_string(), Value::from(file.writable));
            if let Some(error) = &file.parse_error {
                entry.insert("parse_error".to_string(), Value::from(error.clone()));
            }
            Value::Object(entry)
        })
        .collect();

    LoweredSettings {
        effective_json: to_json_or_empty_object(&snapshot.effective),
        provenance_json: to_json_or_empty_object(&provenance),
        files_json: serde_json::to_string(&files).unwrap_or_else(|_| "[]".to_string()),
        active_json: to_json_or_empty_object(&snapshot.active),
        locked: snapshot.locked.clone(),
    }
}

/// Serialize a map, degrading to `{}` rather than panicking at the routing
/// seam. (Unreachable in practice: these maps are string-keyed JSON values.)
fn to_json_or_empty_object<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Four layers could each define the same key; provenance must point at
    /// the layer the priority order actually produced, not a hardcoded
    /// string. Priority (highest first among file layers): local > project >
    /// user. If `FILE_LAYERS` were reversed this assertion would fail,
    /// because "from-local" would no longer be the value that wins the merge.
    #[test]
    fn provenance_names_the_layer_the_merged_value_actually_came_from() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let project = dir.path().join("repo");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(project.join(branding::DOT_DIR)).unwrap();

        std::fs::write(home.join("settings.json"), r#"{"outputStyle":"from-user"}"#).unwrap();
        std::fs::write(
            project.join(branding::DOT_DIR).join("settings.json"),
            r#"{"outputStyle":"from-project"}"#,
        )
        .unwrap();
        std::fs::write(
            project.join(branding::DOT_DIR).join("settings.local.json"),
            r#"{"outputStyle":"from-local"}"#,
        )
        .unwrap();

        let paths = SettingsPaths {
            lingxi_home: home,
            project_dir: project,
        };
        let active = BTreeMap::new();
        let locked = vec!["outputStyle".to_string()];
        let snap = build_snapshot(&paths, active, locked.clone());

        assert_eq!(
            snap.effective.get("outputStyle").and_then(|v| v.as_str()),
            Some("from-local"),
            "local must win over project and user"
        );
        assert_eq!(
            snap.provenance.get("outputStyle"),
            Some(&SettingsLayer::Local),
            "provenance must name Local, the layer the winning value came from"
        );
        assert_eq!(
            snap.locked, locked,
            "locked must be threaded through from the caller, not hardcoded"
        );
    }

    /// Only the layers a user session could actually write are writable
    /// destinations. Reject everything else, naming both the rejected value
    /// and the writable set so the error is actionable.
    #[test]
    fn writable_path_rejects_non_writable_destinations() {
        let paths = SettingsPaths {
            lingxi_home: "/home/u/.lingxi".into(),
            project_dir: "/work/repo".into(),
        };
        let err = writable_path(&paths, SettingsLayer::Managed).unwrap_err();
        assert!(
            err.to_lowercase().contains("managed"),
            "error must name the rejected value, got: {err}"
        );
        assert!(
            err.to_lowercase().contains("user")
                && err.to_lowercase().contains("project")
                && err.to_lowercase().contains("local"),
            "error must name the writable set, got: {err}"
        );
    }

    /// The three file layers must each resolve to their real on-disk path
    /// when writable.
    #[test]
    fn writable_path_resolves_the_three_writable_layers() {
        let home: std::path::PathBuf = "/home/u/.lingxi".into();
        let project: std::path::PathBuf = "/work/repo".into();
        let paths = SettingsPaths {
            lingxi_home: home.clone(),
            project_dir: project.clone(),
        };

        assert_eq!(
            writable_path(&paths, SettingsLayer::User).unwrap(),
            home.join("settings.json")
        );
        assert_eq!(
            writable_path(&paths, SettingsLayer::Project).unwrap(),
            project.join(branding::DOT_DIR).join("settings.json")
        );
        assert_eq!(
            writable_path(&paths, SettingsLayer::Local).unwrap(),
            project.join(branding::DOT_DIR).join("settings.local.json")
        );
    }

    /// The lowering must keep `active` DISTINCT from `effective` — that
    /// distinction is the whole point of the field: an on-disk edit that the
    /// running session has not adopted shows up as the two disagreeing. It
    /// must also name each layer with its wire spelling and carry a parse
    /// error through to the file entry.
    #[test]
    fn lowering_keeps_active_distinct_from_effective_and_names_each_layer() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let project = dir.path().join("repo");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(project.join(branding::DOT_DIR)).unwrap();
        std::fs::write(home.join("settings.json"), r#"{"model":"on-disk"}"#).unwrap();
        std::fs::write(
            project.join(branding::DOT_DIR).join("settings.local.json"),
            "{ not json",
        )
        .unwrap();

        let paths = SettingsPaths {
            lingxi_home: home,
            project_dir: project,
        };
        let mut active = BTreeMap::new();
        active.insert("model".to_string(), Value::from("loaded-at-startup"));
        let lowered = lower_snapshot(&build_snapshot(&paths, active, Vec::new()));

        let effective: Value = serde_json::from_str(&lowered.effective_json).unwrap();
        let active: Value = serde_json::from_str(&lowered.active_json).unwrap();
        assert_eq!(effective["model"], "on-disk");
        assert_eq!(
            active["model"], "loaded-at-startup",
            "active must report what the session loaded, not re-report the files"
        );

        let provenance: Value = serde_json::from_str(&lowered.provenance_json).unwrap();
        assert_eq!(
            provenance["model"], "user",
            "the wire spelling of the layer, lowercase, not the Rust Debug name"
        );

        let files: Value = serde_json::from_str(&lowered.files_json).unwrap();
        let broken = files
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["layer"] == "local")
            .expect("the local layer must be reported even when it fails to parse");
        assert_eq!(broken["writable"], false);
        assert!(
            broken["parse_error"].is_string(),
            "the parse error must reach the wire, got {broken}"
        );
    }

    /// A file layer with syntactically broken JSON must not silently drop
    /// out of the snapshot: it is reported as existing but not writable, with
    /// the parse error preserved, and it must not clobber the keys the other
    /// layers contributed.
    #[test]
    fn broken_layer_reports_parse_error_and_is_not_writable() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let project = dir.path().join("repo");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(project.join(branding::DOT_DIR)).unwrap();

        std::fs::write(home.join("settings.json"), r#"{"model":"opus"}"#).unwrap();
        std::fs::write(
            project.join(branding::DOT_DIR).join("settings.json"),
            "{ not json",
        )
        .unwrap();

        let paths = SettingsPaths {
            lingxi_home: home,
            project_dir: project,
        };
        let snap = build_snapshot(&paths, BTreeMap::new(), Vec::new());

        let project_file = snap
            .files
            .iter()
            .find(|f| f.layer == SettingsLayer::Project)
            .unwrap();
        assert!(project_file.exists);
        assert!(!project_file.writable);
        assert!(project_file.parse_error.is_some());
        // The user layer's key must still be present in the merge.
        assert_eq!(
            snap.effective.get("model").and_then(|v| v.as_str()),
            Some("opus")
        );
    }
}
