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

pub mod continuation;
pub mod error;
pub mod events;
pub mod ids;
pub mod service;
pub mod state;
pub mod storage;
pub mod test_support;
pub mod types;

pub use continuation::{ContinuationSink, NoopContinuationSink, RecordingContinuationSink};
pub use error::{AppError, AppErrorCode};
pub use events::{AppEvent, AppEventObserver, NoopAppEventObserver, RecordingAppEventObserver};
pub use service::AppService;
pub use state::{runtime_transition_allowed, AppState, DRAFT_EDITABLE_STATES};
pub use types::{
    AppCheckpoint, AppCheckpointKind, AppContinuation, AppContinuationKind, AppDesignDraft,
    AppDesignPatch, AppDesignPatchOp, AppDesignSuggestion, AppGenerationProgress,
    AppInteractionKind, AppInteractionRequest, AppInteractions, AppPreview, AppRecord,
    AppRuntimeRecord, AppRuntimeState, AppTemplateKind, AppWorkflowState, DensityLevel,
    DesignValue, APPS_SCHEMA_VERSION,
};
