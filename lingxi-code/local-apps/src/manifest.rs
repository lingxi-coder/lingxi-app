//! Versioned application manifest and filesystem layout.
//!
//! The manifest is the contract shared by generated code, the native data
//! bridge, and the local-apps MCP provider.  Paths are always derived from a
//! validated app id; callers never supply a database or workspace path.

use crate::error::AppError;
use crate::ids;
use crate::permissions::AppCapability;
use crate::types::APPS_SCHEMA_VERSION;
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
/// App-to-conversation mailbox filename.
pub const MAILBOX_FILE: &str = "mailbox.json";

const MAX_MANIFEST_BYTES: u64 = 2 * 1024 * 1024;
/// Maximum UTF-8 byte length for manifest, collection and field display names.
pub const MAX_MANIFEST_DISPLAY_NAME_BYTES: usize = 200;
/// Maximum number of native data collections in one manifest.
pub const MAX_MANIFEST_COLLECTIONS: usize = 64;
/// Maximum number of fields in one native data collection.
pub const MAX_COLLECTION_FIELDS: usize = 128;
/// Maximum number of options declared by an enum field.
pub const MAX_ENUM_OPTIONS: usize = 100;
/// Maximum UTF-8 byte length of one enum option.
pub const MAX_ENUM_OPTION_BYTES: usize = 500;
/// Record properties supplied by the host rather than stored in `document`.
pub const HOST_OWNED_RECORD_FIELD_IDS: [&str; 4] =
    ["recordId", "revision", "createdAtMs", "updatedAtMs"];

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

/// Native host context used to choose a platform-specific generated shell.
///
/// This is deliberately a small, version-tolerant contract: the host owns
/// the values and the generated app treats an absent context as unknown. It
/// is not inferred from the browser user agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceContext {
    /// `ios`, `android`, `desktop`, or `unknown`.
    pub os: String,
    /// `iphone`, `ipad`, `phone`, `tablet`, `desktop`, or `unknown`.
    pub form_factor: String,
    pub viewport: DeviceViewport,
    pub safe_area: DeviceInsets,
    /// `light`, `dark`, or `unknown`.
    pub color_scheme: String,
    pub reduced_motion: bool,
    /// `touch`, `pointer`, `hybrid`, or `unknown`.
    pub input_mode: String,
}

/// CSS-pixel/point viewport dimensions supplied by the native host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceViewport {
    pub width: u32,
    pub height: u32,
}

/// Safe-area insets supplied by the native host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInsets {
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
    pub left: u32,
}

impl DeviceContext {
    fn validate(&self) -> Result<(), AppError> {
        let valid_os = matches!(self.os.as_str(), "ios" | "android" | "desktop" | "unknown");
        let valid_form_factor = matches!(
            self.form_factor.as_str(),
            "iphone" | "ipad" | "phone" | "tablet" | "desktop" | "unknown"
        );
        let valid_color_scheme = matches!(self.color_scheme.as_str(), "light" | "dark" | "unknown");
        let valid_input_mode = matches!(
            self.input_mode.as_str(),
            "touch" | "pointer" | "hybrid" | "unknown"
        );
        if !valid_os || !valid_form_factor || !valid_color_scheme || !valid_input_mode {
            return Err(AppError::InvalidRequest(
                "manifest deviceContext contains an unsupported platform value".into(),
            ));
        }
        if self.viewport.width == 0 || self.viewport.height == 0 {
            return Err(AppError::InvalidRequest(
                "manifest deviceContext viewport must be non-zero".into(),
            ));
        }
        let valid_pair = matches!(
            (self.os.as_str(), self.form_factor.as_str()),
            ("ios", "iphone")
                | ("ios", "ipad")
                | ("android", "phone")
                | ("android", "tablet")
                | ("desktop", "desktop")
                | ("unknown", "unknown")
        );
        if !valid_pair {
            return Err(AppError::InvalidRequest(
                "manifest deviceContext os and formFactor do not agree".into(),
            ));
        }
        Ok(())
    }
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
    /// Native data collections.
    #[serde(default)]
    pub collections: Vec<DataCollectionSchema>,
    /// HTTPS hostnames the app may ask the native network bridge to access.
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    /// Host capabilities the app may request at runtime. Declared by the
    /// confirmed plan; a pre-capability manifest deserializes as empty,
    /// meaning "no device/LLM capability was ever declared".
    #[serde(default)]
    pub capabilities: Vec<AppCapability>,
    /// Host-derived context captured when the generated target was confirmed.
    /// Older manifests omit this field and remain valid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_context: Option<DeviceContext>,
}

impl AppManifest {
    /// Build the initial native contract for a newly-created application.
    /// Collections start empty — the agent fills them in as it designs the
    /// app's data model.
    #[must_use]
    pub fn for_new_app(app_id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            schema_version: APPS_SCHEMA_VERSION,
            app_id: app_id.into(),
            revision: 0,
            name: name.into(),
            collections: Vec::new(),
            allowed_domains: Vec::new(),
            capabilities: Vec::new(),
            device_context: None,
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
        if self.name.len() > MAX_MANIFEST_DISPLAY_NAME_BYTES {
            return Err(AppError::InvalidRequest(format!(
                "manifest name exceeds {MAX_MANIFEST_DISPLAY_NAME_BYTES} bytes"
            )));
        }
        if self.collections.len() > MAX_MANIFEST_COLLECTIONS {
            return Err(AppError::InvalidRequest(format!(
                "manifest has more than {MAX_MANIFEST_COLLECTIONS} collections"
            )));
        }

        let mut collection_ids = BTreeSet::new();
        for collection in &self.collections {
            validate_identifier("collection", &collection.id)?;
            if collection.name.trim().is_empty()
                || collection.name.len() > MAX_MANIFEST_DISPLAY_NAME_BYTES
            {
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
            if collection.fields.len() > MAX_COLLECTION_FIELDS {
                return Err(AppError::InvalidRequest(format!(
                    "collection {:?} has more than {MAX_COLLECTION_FIELDS} fields",
                    collection.id
                )));
            }
            let mut field_ids = BTreeSet::new();
            for field in &collection.fields {
                if HOST_OWNED_RECORD_FIELD_IDS.contains(&field.id.as_str()) {
                    return Err(AppError::InvalidRequest(format!(
                        "field {:?}.{:?} uses host-owned record metadata",
                        collection.id, field.id
                    )));
                }
                validate_identifier("field", &field.id)?;
                if field.label.trim().is_empty()
                    || field.label.len() > MAX_MANIFEST_DISPLAY_NAME_BYTES
                {
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
                        if field.enum_options.is_empty()
                            || field.enum_options.len() > MAX_ENUM_OPTIONS
                        {
                            return Err(AppError::InvalidRequest(format!(
                                "enum field {:?}.{:?} must declare 1..={MAX_ENUM_OPTIONS} options",
                                collection.id, field.id
                            )));
                        }
                        let unique: BTreeSet<_> = field.enum_options.iter().collect();
                        if unique.len() != field.enum_options.len()
                            || field.enum_options.iter().any(|option| {
                                option.is_empty() || option.len() > MAX_ENUM_OPTION_BYTES
                            })
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

        let mut capabilities = BTreeSet::new();
        for capability in &self.capabilities {
            if !capabilities.insert(capability) {
                return Err(AppError::InvalidRequest(format!(
                    "duplicate capability {capability:?}"
                )));
            }
        }
        if let Some(device_context) = &self.device_context {
            device_context.validate()?;
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

    /// Root-relative app-to-conversation mailbox path.
    #[must_use]
    pub fn mailbox_rel(&self) -> PathBuf {
        self.app_dir_rel().join(MAILBOX_FILE)
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
            capabilities: Vec::new(),
            device_context: None,
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
    fn device_context_requires_a_native_platform_form_factor_pair() {
        let mut manifest = manifest();
        manifest.device_context = Some(DeviceContext {
            os: "ios".into(),
            form_factor: "iphone".into(),
            viewport: DeviceViewport {
                width: 393,
                height: 852,
            },
            safe_area: DeviceInsets {
                top: 59,
                right: 0,
                bottom: 34,
                left: 0,
            },
            color_scheme: "light".into(),
            reduced_motion: false,
            input_mode: "touch".into(),
        });
        manifest.validate().unwrap();
        manifest.device_context.as_mut().unwrap().form_factor = "tablet".into();
        assert!(manifest.validate().is_err());
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
    fn rejects_host_owned_record_metadata_as_collection_fields() {
        for reserved in HOST_OWNED_RECORD_FIELD_IDS {
            let mut candidate = manifest();
            candidate.collections[0].fields[0].id = reserved.into();
            let error = candidate.validate().unwrap_err().to_string();
            assert!(
                error.contains("host-owned record metadata"),
                "{reserved:?} returned an unrelated validation error: {error}"
            );
        }
    }

    #[test]
    fn manifest_limits_count_fields_and_utf8_bytes_at_the_documented_boundaries() {
        let mut candidate = manifest();
        candidate.collections = (0..MAX_MANIFEST_COLLECTIONS)
            .map(|index| DataCollectionSchema {
                id: format!("collection_{index}"),
                name: format!("Collection {index}"),
                fields: Vec::new(),
            })
            .collect();
        candidate.validate().unwrap();
        candidate.collections.push(DataCollectionSchema {
            id: "overflow".into(),
            name: "Overflow".into(),
            fields: Vec::new(),
        });
        assert!(candidate.validate().is_err());

        let mut candidate = manifest();
        candidate.collections[0].fields = (0..MAX_COLLECTION_FIELDS)
            .map(|index| DataFieldSchema {
                id: format!("field_{index}"),
                label: "x".repeat(MAX_MANIFEST_DISPLAY_NAME_BYTES),
                kind: DataFieldKind::Text,
                required: false,
                enum_options: Vec::new(),
            })
            .collect();
        candidate.validate().unwrap();

        candidate.collections[0].fields.push(DataFieldSchema {
            id: "overflow".into(),
            label: "Overflow".into(),
            kind: DataFieldKind::Text,
            required: false,
            enum_options: Vec::new(),
        });
        assert!(candidate.validate().is_err());

        let mut candidate = manifest();
        candidate.collections[0].fields[0].label = "é".repeat(100);
        candidate.collections[0].fields[0].enum_options = vec!["é".repeat(250)];
        candidate.validate().unwrap();

        candidate.collections[0].fields[0].label.push('é');
        assert!(candidate.validate().is_err());

        let mut candidate = manifest();
        candidate.collections[0].fields[0].enum_options = vec!["é".repeat(251)];
        assert!(candidate.validate().is_err());
    }

    #[test]
    fn an_old_manifest_json_without_capabilities_loads_as_empty() {
        // The exact on-disk shape every pre-capability app already has. It
        // must keep loading (as "no device capabilities declared") without
        // any migration step.
        let json = r#"{
  "schemaVersion": 1,
  "appId": "abcd1234",
  "revision": 3,
  "name": "Tasks",
  "collections": [],
  "allowedDomains": ["api.example.com"]
}"#;
        let loaded: AppManifest = serde_json::from_str(json).unwrap();
        loaded.validate().unwrap();
        assert!(loaded.capabilities.is_empty());
    }

    #[test]
    fn capabilities_round_trip_through_save_and_load() {
        let root = tempfile::tempdir().unwrap();
        let layout = AppLayout::new(root.path(), "abcd1234").unwrap();
        let mut expected = manifest();
        expected.capabilities = vec![
            crate::permissions::AppCapability::Camera,
            crate::permissions::AppCapability::Microphone,
            crate::permissions::AppCapability::Llm,
        ];
        save_manifest(&layout, &expected).unwrap();
        assert_eq!(load_manifest(&layout).unwrap(), expected);
    }

    #[test]
    fn rejects_a_duplicate_capability() {
        let mut candidate = manifest();
        candidate.capabilities = vec![
            crate::permissions::AppCapability::Camera,
            crate::permissions::AppCapability::Camera,
        ];
        assert!(candidate.validate().is_err());
    }

    #[test]
    fn a_new_app_manifest_declares_no_collections() {
        let manifest = AppManifest::for_new_app("notes", "Notes");
        assert!(
            manifest.collections.is_empty(),
            "collections now come from the LLM plan, not from a template"
        );
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
