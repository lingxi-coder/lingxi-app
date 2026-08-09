//! Local Apps (phase 1) — shared core of the on-device "Apps" capability.
//!
//! Users design small local Next.js apps through an agent-guided wizard; the
//! apps later run inside the on-device Alpine/PRoot runtime. Phase 1 is ONLY
//! the shared foundation: the data model, the designer-workflow and runtime
//! state machines, atomic on-disk storage, resumable human-gate continuations,
//! and [`AppService`] as the single source of truth. There is NO Node/Next
//! execution, NO MCP tool surface, and NO UI here — the runtime arrives in
//! phase 4 and git checkpoints in phase 5.
//!
//! Layer boundaries: this crate knows nothing about the client protocol. The
//! engine maps [`events::AppEvent`]s onto client events and [`error::AppError`]s
//! onto `AppOperationFailed { code, message }`.
//!
//! Storage layout under the injected data root (all writes atomic via
//! `traits::rooted_fs`; every file carries `schemaVersion`):
//!
//! ```text
//! apps/index.json                                  — { schemaVersion, apps: [AppRecord] }
//! apps/<app-id>/runtime.json                       — AppRuntimeRecord
//! apps/<app-id>/interactions.json                  — pending gate + continuation queue
//! apps/<app-id>/workspace/.lingxi/app.json         — app-scoped AppRecord mirror
//! apps/<app-id>/workspace/.lingxi/design-spec.json — AppDesignDraft
//! ```

#![forbid(unsafe_code)]

pub mod checkpoints;
pub mod continuation;
pub mod data;
pub mod error;
pub mod events;
pub mod generation;
pub mod ids;
pub mod mailbox;
pub mod manifest;
pub mod permissions;
pub mod questionnaire;
pub mod service;
pub mod source_validator;
pub mod state;
pub mod storage;
pub mod test_support;
pub mod types;

pub use checkpoints::AppCheckpointStore;
pub use continuation::{ContinuationSink, NoopContinuationSink, RecordingContinuationSink};
pub use data::{
    AppDataStore, DataFilter, DataFilterOperator, DataMigrationPreview, DataMigrationResult,
    DataMutation, DataMutationResult, DataPage, DataQuery, DataRecord, DataSchemaState,
    DataSortDirection, DataSortKey, DATA_SCHEMA_VERSION, MAX_MUTATION_BATCH_SIZE,
    MAX_QUERY_PAGE_SIZE,
};
pub use error::{AppError, AppErrorCode};
pub use events::{
    AppEvent, AppEventFanout, AppEventObserver, AppEventSubscription, NoopAppEventObserver,
    RecordingAppEventObserver,
};
pub use generation::{
    AppGenerationCoordinator, AppGenerationExecutor, GenerationJob, GenerationJobKey,
    GenerationJobObserver, GenerationJobStatus, GenerationRequest, GenerationRequestKind,
    NoopGenerationJobObserver,
};
pub use manifest::{
    load_manifest, save_manifest, AppLayout, AppManifest, DataCollectionSchema, DataFieldKind,
    DataFieldSchema,
};
pub use permissions::{
    load_permissions, save_permissions, AppCapability, AppPermissions, PermissionDecision,
    SessionPermissions,
};
pub use questionnaire::{
    validate_answers, validate_plan, validate_questionnaire, AppDesignField,
    AppDesignFieldOption, AppDesignFieldType, AppDesignStep, AppPlan,
};
pub use service::AppService;
pub use source_validator::{validate_workspace_source, WorkspaceSourcePolicy, WRITABLE_ROOTS};
pub use state::{runtime_transition_allowed, AppState, DRAFT_EDITABLE_STATES};
pub use types::{
    AppCheckpoint, AppCheckpointKind, AppContinuation, AppContinuationKind, AppDesignDraft,
    AppDesignPatch, AppDesignPatchOp, AppDesignSuggestion, AppGenerationProgress,
    AppInteractionKind, AppInteractionRequest, AppInteractions, AppPreview, AppRecord,
    AppRuntimeMode, AppRuntimeRecord, AppRuntimeState, AppWorkflowState, DensityLevel,
    DesignValue, APPS_SCHEMA_VERSION,
};
