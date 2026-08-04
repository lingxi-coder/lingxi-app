//! Versioned application manifest and filesystem layout.
//!
//! The manifest is the contract shared by generated code, the native data
//! bridge, and the local-apps MCP provider.  Paths are always derived from a
//! validated app id; callers never supply a database or workspace path.

use crate::error::AppError;
use crate::ids;
use crate::types::{AppTemplateKind, APPS_SCHEMA_VERSION};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use traits::rooted_fs::{self, AtomicWriteOptions};
use traits::FsError;

/// Manifest filename under `workspace/.lingxi`.
pub const APP_MANIFEST_FILE: &str = "app.manifest.json";
/// App-private data directory.
pub const DATA_DIR: &str = "data";
/// `SQLite` database filename.
pub const DATA_DATABASE_FILE: &str = "app.sqlite";
/// Build output root.
pub const BUILD_DIR: &str = "build";
/// Static Store/Play build output.
pub const STORE_BUILD_DIR: &str = "store";
/// Full/Direct Next production build output.
pub const FULL_BUILD_DIR: &str = "full";
/// App-private log directory.
pub const LOGS_DIR: &str = "logs";
/// Runtime state filename.
pub const RUNTIME_STATE_FILE: &str = "runtime.json";
/// Persisted capability decisions filename.
pub const PERMISSIONS_FILE: &str = "permissions.json";
/// Durable generation queue filename.
pub const GENERATION_JOBS_FILE: &str = "generation-jobs.json";

const MAX_MANIFEST_BYTES: u64 = 2 * 1024 * 1024;

/// Supported native collection field types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataFieldKind {
    /// Single-line string.
    Text,
    /// Multi-line string.
    LongText,
    /// Signed 64-bit integer.
    Integer,
    /// Finite JSON number.
    Decimal,
    /// Boolean value.
    Boolean,
    /// ISO-8601/RFC-3339-shaped timestamp string.
    DateTime,
    /// One of the field's declared options.
    Enum,
    /// Opaque native image reference string.
    ImageRef,
}

/// One field in a native data collection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataFieldSchema {
    /// Stable programmatic field id.
    pub id: String,
    /// User-facing field label.
    pub label: String,
    /// Stored value type.
    pub kind: DataFieldKind,
    /// Whether every newly written record must contain this field.
    #[serde(default)]
    pub required: bool,
    /// Allowed values when `kind` is [`DataFieldKind::Enum`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enum_options: Vec<String>,
}

/// A collection exposed through the controlled native record API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataCollectionSchema {
    /// Stable programmatic collection id.
    pub id: String,
    /// User-facing collection name.
    pub name: String,
    /// Record fields, in designer order.
    pub fields: Vec<DataFieldSchema>,
}

/// Versioned local application manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppManifest {
    /// Persisted local-app schema version.
    pub schema_version: u32,
    /// Stable app id bound by the native host.
    pub app_id: String,
    /// Monotonic manifest revision.
    pub revision: u64,
    /// User-facing app name.
    pub name: String,
    /// Scaffold template family.
    pub template: AppTemplateKind,
    /// Native data collections.
    #[serde(default)]
    pub collections: Vec<DataCollectionSchema>,
    /// HTTPS hostnames the app may ask the native network bridge to access.
    #[serde(default)]
    pub allowed_domains: Vec<String>,
}

impl AppManifest {
    /// Build the initial native contract for a newly-created application.
    /// Template-specific collection ids are stable from the first write so
    /// generated code and the data bridge never need to guess them.
    #[must_use]
    pub fn for_new_app(
        app_id: impl Into<String>,
        name: impl Into<String>,
        template: AppTemplateKind,
    ) -> Self {
        let collection = match template {
            AppTemplateKind::Dashboard => Some(("records", "Records")),
            AppTemplateKind::CrudTracker => Some(("items", "Items")),
            AppTemplateKind::ContentShowcase => Some(("entries", "Entries")),
            // Form history is a designer opt-in; do not create it before the
            // user chooses to retain submissions.
            AppTemplateKind::FormUtility => None,
        };
        Self {
            schema_version: APPS_SCHEMA_VERSION,
            app_id: app_id.into(),
            revision: 0,
            name: name.into(),
            template,
            collections: collection
                .map(|(id, name)| DataCollectionSchema {
                    id: id.to_string(),
                    name: name.to_string(),
                    fields: Vec::new(),
                })
                .into_iter()
                .collect(),
            allowed_domains: Vec::new(),
        }
    }

    /// Validate ids, field definitions, and declared network domains.
    pub fn validate(&self) -> Result<(), AppError> {
        ids::validate_app_id(&self.app_id)?;
        if self.schema_version != APPS_SCHEMA_VERSION {
            return Err(AppError::InvalidRequest(format!(
                "manifest schemaVersion {} is unsupported (expected {APPS_SCHEMA_VERSION})",
                self.schema_version
            )));
        }
        if self.name.trim().is_empty() {
            return Err(AppError::InvalidRequest(
                "manifest name must not be empty".into(),
            ));
        }
        if self.name.len() > 200 {
            return Err(AppError::InvalidRequest(
                "manifest name exceeds 200 bytes".into(),
            ));
        }
        if self.collections.len() > 64 {
            return Err(AppError::InvalidRequest(
                "manifest has more than 64 collections".into(),
            ));
        }

        let mut collection_ids = BTreeSet::new();
        for collection in &self.collections {
            validate_identifier("collection", &collection.id)?;
            if collection.name.trim().is_empty() || collection.name.len() > 200 {
                return Err(AppError::InvalidRequest(format!(
                    "collection {:?} has an invalid display name",
                    collection.id
                )));
            }
            if !collection_ids.insert(collection.id.as_str()) {
                return Err(AppError::InvalidRequest(format!(
                    "duplicate collection id {:?}",
                    collection.id
                )));
            }
            if collection.fields.len() > 128 {
                return Err(AppError::InvalidRequest(format!(
                    "collection {:?} has more than 128 fields",
                    collection.id
                )));
            }
            let mut field_ids = BTreeSet::new();
            for field in &collection.fields {
                validate_identifier("field", &field.id)?;
                if field.label.trim().is_empty() || field.label.len() > 200 {
                    return Err(AppError::InvalidRequest(format!(
                        "field {:?}.{:?} has an invalid label",
                        collection.id, field.id
                    )));
                }
                if !field_ids.insert(field.id.as_str()) {
                    return Err(AppError::InvalidRequest(format!(
                        "duplicate field id {:?} in collection {:?}",
                        field.id, collection.id
                    )));
                }
                match field.kind {
                    DataFieldKind::Enum => {
                        if field.enum_options.is_empty() || field.enum_options.len() > 100 {
                            return Err(AppError::InvalidRequest(format!(
                                "enum field {:?}.{:?} must declare 1..=100 options",
                                collection.id, field.id
                            )));
                        }
                        let unique: BTreeSet<_> = field.enum_options.iter().collect();
                        if unique.len() != field.enum_options.len()
                            || field
                                .enum_options
                                .iter()
                                .any(|option| option.is_empty() || option.len() > 500)
                        {
                            return Err(AppError::InvalidRequest(format!(
                                "enum field {:?}.{:?} has duplicate or invalid options",
                                collection.id, field.id
                            )));
                        }
                    }
                    _ if !field.enum_options.is_empty() => {
                        return Err(AppError::InvalidRequest(format!(
                            "non-enum field {:?}.{:?} cannot declare enum options",
                            collection.id, field.id
                        )));
                    }
                    _ => {}
                }
            }
        }

        let mut domains = BTreeSet::new();
        for domain in &self.allowed_domains {
            validate_domain(domain)?;
            if !domains.insert(domain.as_str()) {
                return Err(AppError::InvalidRequest(format!(
                    "duplicate allowed domain {domain:?}"
                )));
            }
        }
        Ok(())
    }

    /// Find a collection by stable id.
    #[must_use]
    pub fn collection(&self, id: &str) -> Option<&DataCollectionSchema> {
        self.collections
            .iter()
            .find(|collection| collection.id == id)
    }

    /// Stable SHA-256 of the serialized manifest contract.
    pub fn hash(&self) -> Result<String, AppError> {
        self.validate()?;
        let bytes = serde_json::to_vec(self)
            .map_err(|error| AppError::Io(format!("serialize app manifest: {error}")))?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }
}

/// Absolute and root-relative paths for one app.
#[derive(Debug, Clone)]
pub struct AppLayout {
    root: PathBuf,
    app_id: String,
}

impl AppLayout {
    /// Construct a layout from a trusted profile root and validated app id.
    pub fn new(root: impl Into<PathBuf>, app_id: impl Into<String>) -> Result<Self, AppError> {
        let app_id = app_id.into();
        ids::validate_app_id(&app_id)?;
        Ok(Self {
            root: root.into(),
            app_id,
        })
    }

    /// Trusted profile data root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Stable app id.
    #[must_use]
    pub fn app_id(&self) -> &str {
        &self.app_id
    }

    /// Root-relative `apps/<id>` directory.
    #[must_use]
    pub fn app_dir_rel(&self) -> PathBuf {
        crate::storage::app_dir_rel(&self.app_id)
    }

    /// Root-relative workspace directory.
    #[must_use]
    pub fn workspace_rel(&self) -> PathBuf {
        crate::storage::workspace_dir_rel(&self.app_id)
    }

    /// Root-relative manifest path.
    #[must_use]
    pub fn manifest_rel(&self) -> PathBuf {
        self.workspace_rel()
            .join(crate::storage::APP_STATE_DIR)
            .join(APP_MANIFEST_FILE)
    }

    /// Root-relative `SQLite` database path.
    #[must_use]
    pub fn database_rel(&self) -> PathBuf {
        self.app_dir_rel().join(DATA_DIR).join(DATA_DATABASE_FILE)
    }

    /// Absolute `SQLite` database path.
    #[must_use]
    pub fn database_path(&self) -> PathBuf {
        self.root.join(self.database_rel())
    }

    /// Root-relative build directory for a channel (`store` or `full`).
    #[must_use]
    pub fn build_rel(&self, full: bool) -> PathBuf {
        self.app_dir_rel().join(BUILD_DIR).join(if full {
            FULL_BUILD_DIR
        } else {
            STORE_BUILD_DIR
        })
    }

    /// Root-relative log directory.
    #[must_use]
    pub fn logs_rel(&self) -> PathBuf {
        self.app_dir_rel().join(LOGS_DIR)
    }

    /// Root-relative runtime state path.
    #[must_use]
    pub fn runtime_rel(&self) -> PathBuf {
        self.app_dir_rel().join(RUNTIME_STATE_FILE)
    }

    /// Root-relative permissions path.
    #[must_use]
    pub fn permissions_rel(&self) -> PathBuf {
        self.app_dir_rel().join(PERMISSIONS_FILE)
    }

    /// Root-relative durable generation jobs path.
    #[must_use]
    pub fn generation_jobs_rel(&self) -> PathBuf {
        self.app_dir_rel().join(GENERATION_JOBS_FILE)
    }

    /// Create the complete app directory skeleton with private permissions.
    pub fn initialize(&self) -> Result<(), AppError> {
        std::fs::create_dir_all(&self.root).map_err(|error| {
            AppError::Io(format!(
                "create app data root {}: {error}",
                self.root.display()
            ))
        })?;
        for relative in [
            self.workspace_rel().join(crate::storage::APP_STATE_DIR),
            self.app_dir_rel().join(DATA_DIR),
            self.build_rel(false),
            self.build_rel(true),
            self.logs_rel(),
        ] {
            ensure_private_directory(&self.root, &relative)?;
        }
        Ok(())
    }
}

/// Persist a validated manifest atomically.
pub fn save_manifest(layout: &AppLayout, manifest: &AppManifest) -> Result<(), AppError> {
    manifest.validate()?;
    if layout.app_id != manifest.app_id {
        return Err(AppError::InvalidRequest(format!(
            "manifest app id {:?} does not match layout app id {:?}",
            manifest.app_id, layout.app_id
        )));
    }
    layout.initialize()?;
    let mut body = serde_json::to_vec_pretty(manifest)
        .map_err(|error| AppError::Io(format!("serialize app manifest: {error}")))?;
    body.push(b'\n');
    if body.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(AppError::InvalidRequest(format!(
            "app manifest is {} bytes (limit {MAX_MANIFEST_BYTES})",
            body.len()
        )));
    }
    rooted_fs::atomic_write(
        layout.root(),
        &layout.manifest_rel(),
        &body,
        AtomicWriteOptions::default(),
    )
    .map_err(|error| AppError::from_fs("write app manifest", &error))
}

/// Load and validate one manifest.
pub fn load_manifest(layout: &AppLayout) -> Result<AppManifest, AppError> {
    let body = rooted_fs::read_to_string_limited(
        layout.root(),
        &layout.manifest_rel(),
        MAX_MANIFEST_BYTES,
    )
    .map_err(|error| match error {
        FsError::NotFound(_) => {
            AppError::NotFound(format!("manifest for app {} was not found", layout.app_id))
        }
        other => AppError::from_fs("read app manifest", &other),
    })?;
    let manifest: AppManifest = serde_json::from_str(&body)
        .map_err(|error| AppError::StorageCorrupt(format!("app manifest: {error}")))?;
    manifest
        .validate()
        .map_err(|error| AppError::StorageCorrupt(format!("invalid app manifest: {error}")))?;
    if manifest.app_id != layout.app_id {
        return Err(AppError::StorageCorrupt(format!(
            "manifest app id {:?} does not match directory {:?}",
            manifest.app_id, layout.app_id
        )));
    }
    Ok(manifest)
}

/// Validate a programmatic collection or field identifier.
pub fn validate_identifier(kind: &str, value: &str) -> Result<(), AppError> {
    let bytes = value.as_bytes();
    let valid = !bytes.is_empty()
        && bytes.len() <= 64
        && bytes[0].is_ascii_lowercase()
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_');
    if valid {
        Ok(())
    } else {
        Err(AppError::InvalidRequest(format!(
            "invalid {kind} id {value:?}: must match ^[a-z][a-z0-9_]{{0,63}}$"
        )))
    }
}

/// Validate a manifest network hostname (HTTPS is enforced by the bridge).
pub fn validate_domain(domain: &str) -> Result<(), AppError> {
    let valid = !domain.is_empty()
        && domain.len() <= 253
        && !domain.contains(['/', ':', '@'])
        && domain == domain.to_ascii_lowercase()
        && domain.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        });
    if valid {
        Ok(())
    } else {
        Err(AppError::InvalidRequest(format!(
            "invalid HTTPS domain {domain:?}"
        )))
    }
}

fn ensure_private_directory(root: &Path, relative: &Path) -> Result<(), AppError> {
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(AppError::InvalidRequest(format!(
                "invalid app layout path {}",
                relative.display()
            )));
        };
        current.push(component);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(AppError::StorageCorrupt(format!(
                    "{} is not a real directory",
                    current.display()
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match std::fs::create_dir(&current) {
                    Ok(()) => set_private_permissions(&current)?,
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        let metadata = std::fs::symlink_metadata(&current).map_err(|error| {
                            AppError::Io(format!("inspect {}: {error}", current.display()))
                        })?;
                        if !metadata.is_dir() || metadata.file_type().is_symlink() {
                            return Err(AppError::StorageCorrupt(format!(
                                "{} is not a real directory",
                                current.display()
                            )));
                        }
                    }
                    Err(error) => {
                        return Err(AppError::Io(format!(
                            "create {}: {error}",
                            current.display()
                        )));
                    }
                }
            }
            Err(error) => {
                return Err(AppError::Io(format!(
                    "inspect {}: {error}",
                    current.display()
                )));
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) -> Result<(), AppError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| AppError::Io(format!("secure {}: {error}", path.display())))
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path) -> Result<(), AppError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> AppManifest {
        AppManifest {
            schema_version: APPS_SCHEMA_VERSION,
            app_id: "abcd1234".into(),
            revision: 1,
            name: "Tasks".into(),
            template: AppTemplateKind::CrudTracker,
            collections: vec![DataCollectionSchema {
                id: "items".into(),
                name: "Items".into(),
                fields: vec![DataFieldSchema {
                    id: "status".into(),
                    label: "Status".into(),
                    kind: DataFieldKind::Enum,
                    required: true,
                    enum_options: vec!["todo".into(), "done".into()],
                }],
            }],
            allowed_domains: vec!["api.example.com".into()],
        }
    }

    #[test]
    fn initializes_complete_layout_and_round_trips_manifest() {
        let root = tempfile::tempdir().unwrap();
        let layout = AppLayout::new(root.path(), "abcd1234").unwrap();
        layout.initialize().unwrap();
        for relative in [
            layout.workspace_rel().join(crate::storage::APP_STATE_DIR),
            layout.app_dir_rel().join(DATA_DIR),
            layout.build_rel(false),
            layout.build_rel(true),
            layout.logs_rel(),
        ] {
            assert!(root.path().join(relative).is_dir());
        }
        let expected = manifest();
        save_manifest(&layout, &expected).unwrap();
        assert_eq!(load_manifest(&layout).unwrap(), expected);
        assert_eq!(expected.hash().unwrap().len(), 64);
    }

    #[test]
    fn rejects_duplicate_ids_and_non_https_host_shapes() {
        let mut candidate = manifest();
        candidate.collections.push(candidate.collections[0].clone());
        assert!(candidate.validate().is_err());

        let mut candidate = manifest();
        candidate.allowed_domains = vec!["https://example.com/path".into()];
        assert!(candidate.validate().is_err());
    }

    #[test]
    fn new_app_manifest_uses_stable_template_collection_ids() {
        for (template, expected) in [
            (AppTemplateKind::Dashboard, Some("records")),
            (AppTemplateKind::CrudTracker, Some("items")),
            (AppTemplateKind::ContentShowcase, Some("entries")),
            (AppTemplateKind::FormUtility, None),
        ] {
            let manifest = AppManifest::for_new_app("abcd1234", "App", template);
            manifest.validate().unwrap();
            assert_eq!(
                manifest.collections.first().map(|item| item.id.as_str()),
                expected
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlink_in_layout() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("apps")).unwrap();
        symlink("/tmp", root.path().join("apps/abcd1234")).unwrap();
        let layout = AppLayout::new(root.path(), "abcd1234").unwrap();
        assert!(matches!(
            layout.initialize(),
            Err(AppError::StorageCorrupt(_))
        ));
    }
}
