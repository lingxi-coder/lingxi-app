//! Local Apps — shared core of the on-device "Apps" capability.
//!
//! The conversation agent drives app creation (v3); this crate is the shared
//! foundation underneath it: the data model, the two-state workflow record,
//! the runtime state machine, atomic on-disk storage, git checkpoints, the
//! per-app data store, and [`AppService`] as the single source of truth.
//!
//! Layer boundaries: this crate knows nothing about the client protocol. The
//! engine maps [`events::AppEvent`]s onto client events and [`error::AppError`]s
//! onto `AppOperationFailed { code, message }`.
//!
//! Storage layout under the injected data root (all writes atomic via
//! `traits::rooted_fs`; every file carries `schemaVersion`):
//!
//! ```text
//! apps/index.json                          — { schemaVersion, apps: [AppRecord] }
//! apps/<app-id>/runtime.json               — AppRuntimeRecord
//! apps/<app-id>/workspace/.lingxi/app.json — app-scoped AppRecord mirror
//! ```
//!
//! Legacy pipeline documents (`interactions.json`, `design-spec.json`) from
//! pre-v3 stores are ignored on load and left on disk untouched.

#![forbid(unsafe_code)]

pub mod checkpoints;
pub mod data;
pub mod error;
pub mod events;
pub mod ids;
pub mod mailbox;
pub mod manifest;
pub mod permissions;
pub mod service;
pub mod state;
pub mod storage;
pub mod test_support;
pub mod types;

pub use checkpoints::AppCheckpointStore;
pub use data::{
    AppDataStore, DataFilter, DataFilterOperator, DataMigrationPreview, DataMigrationResult,
    DataMutation, DataMutationResult, DataPage, DataQuery, DataRecord, DataSchemaState,
    DataSortDirection, DataSortKey, DATA_SCHEMA_VERSION, MAX_FILTER_IN_VALUES,
    MAX_MUTATION_BATCH_SIZE, MAX_QUERY_FILTERS, MAX_QUERY_PAGE_SIZE, MAX_RECORD_ID_BYTES,
};
pub use error::{AppError, AppErrorCode};
pub use events::{
    AppEvent, AppEventFanout, AppEventObserver, AppEventSubscription, NoopAppEventObserver,
    RecordingAppEventObserver,
};
pub use manifest::{
    load_manifest, save_manifest, AppLayout, AppManifest, DataCollectionSchema, DataFieldKind,
    DataFieldSchema, DeviceContext, DeviceInsets, DeviceViewport, WORKSPACE_SETTINGS_LOCAL_FILE,
};
pub use permissions::{
    load_permissions, save_permissions, save_workspace_permission_settings, AppCapability,
    AppPermissions, PermissionDecision, SessionPermissions, LOCAL_APP_WORKSPACE_PERMISSION_RULES,
};
pub use service::AppService;
pub use state::{runtime_transition_allowed, AppState};
pub use types::{
    AppCheckpoint, AppCheckpointKind, AppDependencyRecord, AppDependencyState, AppRecord,
    AppRuntimeMode, AppRuntimeRecord, AppRuntimeState, AppWorkflowState, APPS_SCHEMA_VERSION,
    DEFAULT_GIT_VERSION_CONTROL,
};
