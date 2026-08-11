//! Local-apps domain ⇄ client-protocol bridge (phase 1, T3).
//!
//! The `local-apps` core deliberately knows nothing about the client protocol
//! (its module doc: "the engine maps `AppEvent`s onto client events and
//! `AppError`s onto `AppOperationFailed`"). This module is that mapping:
//!
//! - [`AppEmissionQueue`] is the bridge-owned ordered channel every app-surface
//!   client event rides to the connection's single
//!   [`client_adapter::ClientEventSink`] (the same channel every other client
//!   event rides — governing decision §0.2): domain events enqueued by
//!   [`SinkAppEventObserver`], plus the engine-synthesized events that have no
//!   domain twin (`AppOperationFailed`, the `ListAppCheckpoints` reply rows);
//! - [`SinkAppEventObserver`] is the [`AppEventObserver`] installed on the
//!   engine-owned `AppService` at build time; its `on_event` is a cheap
//!   ordered ENQUEUE onto that channel (never an await of foreign code);
//! - the `lower_*` helpers turn core types into their wire DTOs;
//! - the `raise_*` helpers turn inbound command DTOs back into core /
//!   engine-side types. The DTO enums are `#[non_exhaustive]`, so raising is
//!   fallible: a variant this engine version does not know fails typed with
//!   [`AppError::InvalidRequest`] instead of being silently dropped (or, for
//!   `CreateApp`'s origin, silently laundered into a library create).
//!
//! uniffi-gated like the `host` module — it names the client-protocol DTO
//! surface, which is pulled only under that feature.

use std::sync::Arc;

use async_trait::async_trait;
use client_adapter::ClientEventSink;
use client_protocol::events::ClientEvent;
use client_protocol::local_apps::{
    AppCapabilityKindDto, AppCheckpointDto, AppCheckpointKindDto, AppCreateOriginDto,
    AppDataCollectionDto, AppDataFieldDto, AppDataFieldTypeDto, AppDetailsDto, AppErrorCodeDto,
    AppManifestDto, AppRecordDto, AppRuntimeDetailsDto, AppRuntimeModeDto, DeviceContextDto,
    DeviceInsetsDto, DeviceViewportDto,
    AppRuntimeRecoveryStateDto, AppRuntimeStateDto, AppWorkflowStateDto,
};
use local_apps::{
    load_manifest, AppCapability, AppCheckpoint, AppCheckpointKind, AppError, AppErrorCode,
    AppEvent, AppEventObserver, AppLayout, AppManifest, AppRecord, AppRuntimeRecord,
    AppRuntimeState, AppService, AppWorkflowState, DataCollectionSchema, DataFieldKind,
    DataFieldSchema,
};
use tokio::sync::{mpsc, oneshot};

/// One item on the bridge's ordered app-emission channel (see
/// [`AppEmissionQueue`]).
pub(crate) enum AppEmission {
    /// A domain event, enqueued by [`SinkAppEventObserver::on_event`] while
    /// the service's emission-order guard is held (so channel order extends
    /// the service's commit order).
    Domain(AppEvent),
    /// An engine-synthesized `App*` client event with no domain twin
    /// (`AppOperationFailed`, the `ListAppCheckpoints` reply rows).
    Engine(ClientEvent),
    /// Barrier: acknowledged by the forwarder once every emission enqueued
    /// before it has been emitted on the sink. Only the test barrier
    /// ([`AppEmissionQueue::flush`]) constructs it today, so the non-test
    /// lib build must not flag it dead.
    #[cfg_attr(not(test), allow(dead_code))]
    Flush(oneshot::Sender<()>),
}

/// Sender half of the bridge-owned app-emission channel, plus the ONE
/// detached forwarder task (spawned once at engine build) that drains it.
///
/// INVARIANT (this channel is what makes it hold): **no lock is held while
/// foreign listener code runs; ordering = channel order = commit order.**
///
/// - `AppService` calls [`SinkAppEventObserver::on_event`] from its emission
///   tasks WHILE holding its emission-order guard; `on_event` only enqueues
///   and returns, so the guard is never held across `sink.emit` (a listener
///   that drives another app command from inside its event callback — a
///   cross-task cycle the core's task-local reentrancy panic cannot see —
///   completes instead of silently deadlocking the whole app surface).
/// - Enqueueing under the guard preserves commit order INTO the channel;
///   the single consumer preserves it OUT to the sink. Engine-synthesized
///   events ride the same channel (see [`Self::emit_failure`]), so the app
///   surface has one total order.
/// - The forwarder ends when every sender is dropped (engine teardown).
#[derive(Clone)]
pub(crate) struct AppEmissionQueue {
    tx: mpsc::UnboundedSender<AppEmission>,
}

impl AppEmissionQueue {
    /// Create the channel and spawn its single forwarder task on the engine
    /// runtime. The forwarder drains in order, lowers, and awaits
    /// `sink.emit` with no lock held; it exits when the last sender drops.
    pub(crate) fn spawn(runtime: &tokio::runtime::Handle, sink: Arc<dyn ClientEventSink>) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel();
        runtime.spawn(async move {
            while let Some(emission) = rx.recv().await {
                match emission {
                    AppEmission::Domain(event) => {
                        // `None` means `lower_app_event` deliberately dropped
                        // an event with no wire representation yet (its own
                        // doc); skip it and keep the forwarder alive instead
                        // of emitting a placeholder.
                        if let Some(client_event) = lower_app_event(event) {
                            sink.emit(client_event).await;
                        }
                    }
                    AppEmission::Engine(event) => sink.emit(event).await,
                    AppEmission::Flush(ack) => {
                        // A dropped receiver just means the flusher stopped
                        // waiting; the barrier itself has no other effect.
                        let _ = ack.send(());
                    }
                }
            }
        });
        Self { tx }
    }

    /// Ordered enqueue. A send failure means the forwarder ended — i.e. the
    /// engine runtime is tearing down — and matches the pre-channel behavior
    /// (spawned emission tasks dying at shutdown): the late event is dropped.
    pub(crate) fn enqueue(&self, emission: AppEmission) {
        let _ = self.tx.send(emission);
    }

    /// Enqueue one engine-synthesized `App*` client event.
    pub(crate) fn enqueue_engine(&self, event: ClientEvent) {
        self.enqueue(AppEmission::Engine(event));
    }

    /// Lower one typed [`AppError`] onto `AppOperationFailed` and enqueue it
    /// AFTER every event its cause already committed: when the service is
    /// alive, `flush_events` first waits for the emission tasks of every
    /// previously completed call (whose `on_event` enqueues ran under the
    /// emission-order guard), so a failure can never overtake its own cause.
    pub(crate) async fn emit_failure(
        &self,
        service: Option<&AppService>,
        app_id: Option<String>,
        error: &AppError,
    ) {
        if let Some(service) = service {
            service.flush_events().await;
        }
        self.enqueue_engine(ClientEvent::AppOperationFailed {
            app_id,
            code: lower_error_code(error.code()),
            message: error.to_string(),
        });
    }

    /// Barrier: resolves once everything enqueued before it has been emitted
    /// on the sink. (For "everything COMMITTED is delivered", call
    /// `AppService::flush_events` first — commit → enqueue — then this —
    /// enqueue → sink.) Used by the host tests' `drain_events`; the non-test
    /// lib build has no caller, so it must not flag as dead there.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) async fn flush(&self) {
        let (ack_tx, ack_rx) = oneshot::channel();
        self.enqueue(AppEmission::Flush(ack_tx));
        // Err = the forwarder is gone (engine teardown); nothing left to
        // wait for.
        let _ = ack_rx.await;
    }
}

/// [`AppEventObserver`] that hands every domain event to the bridge's ordered
/// emission channel. Installed on the engine-owned `AppService` at build time
/// so service-driven events ride the same outbound channel as command
/// replies.
pub(crate) struct SinkAppEventObserver {
    queue: AppEmissionQueue,
}

impl SinkAppEventObserver {
    /// Observer over the connection's app-emission channel.
    pub(crate) fn new(queue: AppEmissionQueue) -> Self {
        Self { queue }
    }
}

#[async_trait]
impl AppEventObserver for SinkAppEventObserver {
    async fn on_event(&self, event: AppEvent) {
        // Cheap ordered enqueue ONLY. This runs inside the service's emission
        // task while its emission-order guard is held: enqueueing here is
        // what extends commit order into the channel, and returning without
        // awaiting the sink is what keeps foreign listener code outside the
        // guard (see the invariant on [`AppEmissionQueue`]).
        self.queue.enqueue(AppEmission::Domain(event));
    }
}

/// Lower one domain event onto its `App*` [`ClientEvent`] (1:1 by design;
/// `AppOperationFailed` has no domain twin — the engine synthesizes it from
/// typed [`AppError`]s at the command boundary).
///
/// Every variant maps to `Some` today, but the return type stays
/// `Option<ClientEvent>` DELIBERATELY, not just as a historical leftover:
/// the forwarder task that calls this function is spawned once, detached,
/// with its `JoinHandle` discarded (`AppEmissionQueue::spawn` above), so a
/// `panic!`/`todo!()`/`unreachable!()` arm here would silently kill the
/// ENTIRE app-event stream for every app, forever, with no crash and no log
/// (the panicked task's `JoinError` is never awaited, and `enqueue`'s
/// `let _ = self.tx.send(..)` swallows the resulting closed-channel error).
/// Keeping the `Option` signature means the NEXT domain event that outruns
/// its DTO can degrade the same safe way instead of reintroducing that
/// window.
pub(crate) fn lower_app_event(event: AppEvent) -> Option<ClientEvent> {
    match event {
        AppEvent::AppsChanged { apps } => Some(ClientEvent::AppsChanged {
            apps: lower_records(&apps),
        }),
        AppEvent::WorkflowChanged {
            app_id,
            state,
            detail,
        } => Some(ClientEvent::AppWorkflowChanged {
            app_id,
            state: lower_workflow_state(state),
            detail,
        }),
        AppEvent::RuntimeChanged { app_id, runtime } => Some(ClientEvent::AppRuntimeChanged {
            app_id,
            state: lower_runtime_state(runtime.state),
            details: Some(lower_runtime_details(&runtime)),
            last_error: runtime.last_error,
        }),
        AppEvent::CheckpointCreated { app_id, checkpoint } => {
            Some(ClientEvent::AppCheckpointCreated {
                app_id,
                checkpoint: lower_checkpoint(&checkpoint),
            })
        }
    }
}

/// Lower a record list for `AppsChanged`.
pub(crate) fn lower_records(records: &[AppRecord]) -> Vec<AppRecordDto> {
    records.iter().map(lower_record).collect()
}

/// Lower one core [`AppRecord`] to its wire row.
pub(crate) fn lower_record(record: &AppRecord) -> AppRecordDto {
    AppRecordDto {
        id: record.id.clone(),
        name: record.name.clone(),
        brief: record.brief.clone(),
        git_enabled: record.git_enabled,
        created_at_ms: record.created_at_ms,
        updated_at_ms: record.updated_at_ms,
        workflow_state: lower_workflow_state(record.workflow_state),
        conversation_id: record.conversation_id.clone(),
        init_session_id: record.init_session_id.clone(),
        workspace_rel: record.workspace_rel.clone(),
    }
}

/// Lower a typed failure code for `AppOperationFailed`.
pub(crate) fn lower_error_code(code: AppErrorCode) -> AppErrorCodeDto {
    match code {
        AppErrorCode::NotFound => AppErrorCodeDto::NotFound,
        AppErrorCode::RevisionConflict => AppErrorCodeDto::RevisionConflict,
        AppErrorCode::InteractionInvalid => AppErrorCodeDto::InteractionInvalid,
        AppErrorCode::WorkflowStateInvalid => AppErrorCodeDto::WorkflowStateInvalid,
        AppErrorCode::RuntimeBusy => AppErrorCodeDto::RuntimeBusy,
        AppErrorCode::NotYetAvailable => AppErrorCodeDto::NotYetAvailable,
        AppErrorCode::StorageCorrupt => AppErrorCodeDto::StorageCorrupt,
        AppErrorCode::InvalidRequest => AppErrorCodeDto::InvalidRequest,
        AppErrorCode::Io => AppErrorCodeDto::Io,
        AppErrorCode::LlmUnavailable => AppErrorCodeDto::LlmUnavailable,
        AppErrorCode::LlmOutputRejected => AppErrorCodeDto::LlmOutputRejected,
    }
}

fn lower_workflow_state(state: AppWorkflowState) -> AppWorkflowStateDto {
    match state {
        AppWorkflowState::Draft => AppWorkflowStateDto::Draft,
        AppWorkflowState::Ready => AppWorkflowStateDto::Ready,
    }
}

fn lower_runtime_state(state: AppRuntimeState) -> AppRuntimeStateDto {
    match state {
        AppRuntimeState::Stopped => AppRuntimeStateDto::Stopped,
        AppRuntimeState::Starting => AppRuntimeStateDto::Starting,
        AppRuntimeState::Running => AppRuntimeStateDto::Running,
        AppRuntimeState::Stopping => AppRuntimeStateDto::Stopping,
        AppRuntimeState::Failed => AppRuntimeStateDto::Failed,
    }
}

/// Lower one core [`AppCheckpoint`] for `AppCheckpointCreated`.
pub(crate) fn lower_checkpoint(checkpoint: &AppCheckpoint) -> AppCheckpointDto {
    AppCheckpointDto {
        id: checkpoint.id.clone(),
        label: checkpoint.label.clone(),
        kind: lower_checkpoint_kind(checkpoint.kind),
        created_at_ms: checkpoint.created_at_ms,
    }
}

fn lower_checkpoint_kind(kind: AppCheckpointKind) -> AppCheckpointKindDto {
    match kind {
        AppCheckpointKind::ScaffoldCreated => AppCheckpointKindDto::ScaffoldCreated,
        AppCheckpointKind::GenerationValidated => AppCheckpointKindDto::GenerationValidated,
        AppCheckpointKind::PreviewApproved => AppCheckpointKindDto::PreviewApproved,
        AppCheckpointKind::UserApproved => AppCheckpointKindDto::UserApproved,
        AppCheckpointKind::PreRestore => AppCheckpointKindDto::PreRestore,
    }
}

fn lower_data_field(value: DataFieldSchema) -> AppDataFieldDto {
    AppDataFieldDto {
        id: value.id,
        label: value.label,
        field_type: match value.kind {
            DataFieldKind::Text => AppDataFieldTypeDto::Text,
            DataFieldKind::LongText => AppDataFieldTypeDto::LongText,
            DataFieldKind::Integer => AppDataFieldTypeDto::Integer,
            DataFieldKind::Decimal => AppDataFieldTypeDto::Decimal,
            DataFieldKind::Boolean => AppDataFieldTypeDto::Boolean,
            DataFieldKind::DateTime => AppDataFieldTypeDto::DateTime,
            DataFieldKind::Enum => AppDataFieldTypeDto::Enum,
            DataFieldKind::ImageRef => AppDataFieldTypeDto::ImageRef,
        },
        required: value.required,
        options: value.enum_options,
    }
}

fn lower_collection(value: DataCollectionSchema, enabled_by_default: bool) -> AppDataCollectionDto {
    AppDataCollectionDto {
        id: value.id,
        label: value.name,
        fields: value.fields.into_iter().map(lower_data_field).collect(),
        enabled_by_default,
    }
}

fn lower_capability(capability: AppCapability) -> AppCapabilityKindDto {
    match capability {
        AppCapability::DataMutation => AppCapabilityKindDto::DataMutation,
        AppCapability::UiControl => AppCapabilityKindDto::UiControl,
        AppCapability::Camera => AppCapabilityKindDto::Camera,
        AppCapability::PhotoLibrary => AppCapabilityKindDto::PhotoLibrary,
        AppCapability::Microphone => AppCapabilityKindDto::Microphone,
        AppCapability::Location => AppCapabilityKindDto::Location,
        AppCapability::Notifications => AppCapabilityKindDto::Notifications,
        AppCapability::Llm => AppCapabilityKindDto::Llm,
        AppCapability::AgentNotify => AppCapabilityKindDto::AgentNotify,
    }
}

pub(crate) fn lower_runtime_details(runtime: &AppRuntimeRecord) -> AppRuntimeDetailsDto {
    AppRuntimeDetailsDto {
        state: lower_runtime_state(runtime.state),
        mode: runtime.mode.map(|mode| match mode {
            local_apps::AppRuntimeMode::StaticExport => AppRuntimeModeDto::StaticExport,
            local_apps::AppRuntimeMode::NextProduction => AppRuntimeModeDto::NextProduction,
        }),
        loopback_url: runtime.port.map(|port| format!("http://127.0.0.1:{port}")),
        // Unconditionally `None` in this phase, and NOT an oversight: the core
        // `AppRuntimeRecord` records no reason for a non-user-initiated stop
        // (state / mode / port / pid / last_error / updated_at_ms), and
        // `AppRuntimeState` has no suspended state at all.  Every stop the
        // engine can perform — user stop, quota eviction, process exit, a dead
        // static listener — lands in `Stopped`/`Failed` with the detail in
        // `last_error`.  Wiring a real producer therefore starts with a new
        // field on the persisted record, not here; until then no client may
        // treat a suspended runtime as reachable.
        suspension_reason: None,
        recovery_state: Some(match runtime.state {
            AppRuntimeState::Running => AppRuntimeRecoveryStateDto::Recovered,
            _ => AppRuntimeRecoveryStateDto::NotNeeded,
        }),
        last_error: runtime.last_error.clone(),
    }
}

pub(crate) fn lower_manifest(manifest: AppManifest) -> AppManifestDto {
    AppManifestDto {
        schema_version: manifest.schema_version,
        app_id: manifest.app_id,
        name: manifest.name,
        design_revision: manifest.revision,
        collections: manifest
            .collections
            .into_iter()
            .map(|collection| lower_collection(collection, true))
            .collect(),
        allowed_domains: manifest.allowed_domains,
        capabilities: manifest
            .capabilities
            .iter()
            .copied()
            .map(lower_capability)
            .collect(),
        device_context: manifest.device_context.map(|context| DeviceContextDto {
            os: context.os,
            form_factor: context.form_factor,
            viewport: DeviceViewportDto {
                width: context.viewport.width,
                height: context.viewport.height,
            },
            safe_area: DeviceInsetsDto {
                top: context.safe_area.top,
                right: context.safe_area.right,
                bottom: context.safe_area.bottom,
                left: context.safe_area.left,
            },
            color_scheme: context.color_scheme,
            reduced_motion: context.reduced_motion,
            input_mode: context.input_mode,
        }),
    }
}

pub(crate) fn load_manifest_snapshot(
    root: &std::path::Path,
    app_id: &str,
) -> Result<Option<AppManifestDto>, AppError> {
    let layout = AppLayout::new(root, app_id)?;
    match load_manifest(&layout) {
        Ok(manifest) => Ok(Some(lower_manifest(manifest))),
        Err(AppError::NotFound(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Assemble the trimmed detail snapshot (v3): record + manifest + runtime +
/// checkpoints. The designer draft/questionnaire/plan/generation-job fields
/// are gone with the pipeline.
pub(crate) fn lower_details(
    root: &std::path::Path,
    record: &AppRecord,
    runtime: &AppRuntimeRecord,
    checkpoints: &[AppCheckpoint],
) -> Result<AppDetailsDto, AppError> {
    Ok(AppDetailsDto {
        app: lower_record(record),
        manifest: load_manifest_snapshot(root, &record.id)?,
        runtime: lower_runtime_details(runtime),
        checkpoints: checkpoints.iter().map(lower_checkpoint).collect(),
    })
}

/// Engine-side raised form of [`AppCreateOriginDto`]. The core deliberately
/// has no origin type — origin exists only to decide the wire-boundary
/// conversation binding — so the raised value lives here, and
/// `handle_create_app` matches it EXHAUSTIVELY (no wildcard): a future known
/// origin cannot silently inherit either binding rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AppCreateOrigin {
    /// Created from an agent conversation (keeps the `conversation_id`).
    Chat,
    /// Created from the apps library screen (never binds a conversation).
    Library,
}

impl AppCreateOrigin {
    /// The record's conversation binding for this origin
    /// (`AppRecord.conversation_id` doc: "origin: chat"): `Chat` keeps the
    /// caller's binding, `Library` never binds one.
    pub(crate) fn conversation_binding(self, conversation_id: Option<String>) -> Option<String> {
        match self {
            AppCreateOrigin::Chat => conversation_id,
            AppCreateOrigin::Library => None,
        }
    }
}

/// Raise an inbound create-origin DTO to the engine-side origin.
///
/// # Errors
///
/// `invalid_request` for a `#[non_exhaustive]` origin this engine version
/// does not know. The wildcard arm is forced by the cross-crate
/// `#[non_exhaustive]`, but it must FAIL typed — never default: silently
/// treating an unknown future origin as a library create would drop its
/// conversation binding (or whatever semantics the new origin carries).
pub(crate) fn raise_origin(origin: AppCreateOriginDto) -> Result<AppCreateOrigin, AppError> {
    Ok(match origin {
        AppCreateOriginDto::Chat => AppCreateOrigin::Chat,
        AppCreateOriginDto::Library => AppCreateOrigin::Library,
        other => {
            return Err(AppError::InvalidRequest(format!(
                "unsupported app create origin: {other:?}"
            )))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use local_apps::AppWorkflowState;

    #[test]
    fn dto_and_core_agree_on_the_wire_strings() {
        // The DTOs mirror the core enums byte-for-byte on the wire; comparing
        // serde output guards against either side drifting.
        for (core, dto) in [
            (
                serde_json::to_string(&AppWorkflowState::Ready).unwrap(),
                serde_json::to_string(&lower_workflow_state(AppWorkflowState::Ready)).unwrap(),
            ),
            (
                serde_json::to_string(&AppWorkflowState::Draft).unwrap(),
                serde_json::to_string(&lower_workflow_state(AppWorkflowState::Draft)).unwrap(),
            ),
            (
                serde_json::to_string(&AppRuntimeState::Stopping).unwrap(),
                serde_json::to_string(&lower_runtime_state(AppRuntimeState::Stopping)).unwrap(),
            ),
            (
                serde_json::to_string(&AppErrorCode::NotYetAvailable).unwrap(),
                serde_json::to_string(&lower_error_code(AppErrorCode::NotYetAvailable)).unwrap(),
            ),
            (
                serde_json::to_string(&AppCheckpointKind::PreRestore).unwrap(),
                serde_json::to_string(&lower_checkpoint_kind(AppCheckpointKind::PreRestore))
                    .unwrap(),
            ),
        ] {
            assert_eq!(core, dto);
        }
    }

    #[test]
    fn every_error_code_lowers_to_its_dto() {
        for code in [
            AppErrorCode::NotFound,
            AppErrorCode::RevisionConflict,
            AppErrorCode::InteractionInvalid,
            AppErrorCode::WorkflowStateInvalid,
            AppErrorCode::RuntimeBusy,
            AppErrorCode::NotYetAvailable,
            AppErrorCode::StorageCorrupt,
            AppErrorCode::InvalidRequest,
            AppErrorCode::Io,
            AppErrorCode::LlmUnavailable,
            AppErrorCode::LlmOutputRejected,
        ] {
            assert_eq!(
                serde_json::to_string(&code).unwrap(),
                serde_json::to_string(&lower_error_code(code)).unwrap()
            );
        }
    }

    #[test]
    fn workflow_event_lowers_onto_the_client_event() {
        let event = lower_app_event(AppEvent::WorkflowChanged {
            app_id: "abcd1234".into(),
            state: AppWorkflowState::Ready,
            detail: Some("built".into()),
        })
        .expect("WorkflowChanged always has a wire representation");
        match event {
            ClientEvent::AppWorkflowChanged {
                app_id,
                state,
                detail,
            } => {
                assert_eq!(app_id, "abcd1234");
                assert_eq!(state, AppWorkflowStateDto::Ready);
                assert_eq!(detail.as_deref(), Some("built"));
            }
            other => panic!("expected AppWorkflowChanged, got {other:?}"),
        }
    }

    #[test]
    fn raise_origin_maps_both_known_variants_with_their_conversation_semantics() {
        // Chat keeps the caller's conversation binding…
        let chat = raise_origin(AppCreateOriginDto::Chat).expect("chat is a known origin");
        assert_eq!(chat, AppCreateOrigin::Chat);
        assert_eq!(
            chat.conversation_binding(Some("conv-1".into())),
            Some("conv-1".into())
        );
        assert_eq!(chat.conversation_binding(None), None);
        // …and Library never binds one, even when the caller sent an id.
        let library = raise_origin(AppCreateOriginDto::Library).expect("library is a known origin");
        assert_eq!(library, AppCreateOrigin::Library);
        assert_eq!(library.conversation_binding(Some("conv-1".into())), None);
        // The failing wildcard arm cannot be exercised from this crate:
        // `AppCreateOriginDto` is `#[non_exhaustive]` in `client-protocol`,
        // and every variant that exists today is matched above — the arm
        // exists precisely for variants a FUTURE client-protocol version
        // adds, which must fail typed (`invalid_request`) instead of
        // laundering into a library create.
    }

    /// The trimmed detail snapshot carries record + manifest + runtime +
    /// checkpoints and nothing else — a regression that resurrects a
    /// pipeline field fails to compile against `AppDetailsDto`, and a
    /// regression that stops wiring one of the four surviving fields fails
    /// here.
    #[test]
    fn lower_details_assembles_the_trimmed_snapshot() {
        let root = tempfile::tempdir().expect("tempdir");
        let record = AppRecord {
            id: "app00001".into(),
            name: "Habits".into(),
            brief: "a habit tracker".into(),
            git_enabled: true,
            created_at_ms: 1,
            updated_at_ms: 2,
            workflow_state: AppWorkflowState::Draft,
            conversation_id: None,
            init_session_id: None,
            workspace_rel: "apps/app00001/workspace".into(),
        };
        let runtime = AppRuntimeRecord {
            schema_version: local_apps::APPS_SCHEMA_VERSION,
            app_id: "app00001".into(),
            state: AppRuntimeState::Stopped,
            mode: None,
            port: None,
            pid: None,
            last_error: None,
            updated_at_ms: 2,
        };
        let checkpoint = AppCheckpoint {
            id: "cp-1".into(),
            label: "scaffold".into(),
            kind: AppCheckpointKind::ScaffoldCreated,
            created_at_ms: 33,
        };
        let details = lower_details(root.path(), &record, &runtime, std::slice::from_ref(&checkpoint))
            .expect("lowers");
        assert_eq!(details.app, lower_record(&record));
        assert_eq!(
            details.manifest, None,
            "no manifest on disk lowers to None, not an error"
        );
        assert_eq!(details.runtime, lower_runtime_details(&runtime));
        assert_eq!(details.checkpoints, vec![lower_checkpoint(&checkpoint)]);
    }

    /// W4: exact field mapping for EVERY `lower_app_event` arm (one pair per
    /// `AppEvent` variant). The expectation side uses exhaustive struct
    /// literals with a distinct value per field, so a transposed or dropped
    /// field fails here instead of shipping silently.
    #[test]
    fn every_app_event_arm_lowers_field_exact() {
        let record = AppRecord {
            id: "app00001".into(),
            name: "Habits".into(),
            brief: "a habit tracker".into(),
            git_enabled: true,
            created_at_ms: 11,
            updated_at_ms: 22,
            workflow_state: AppWorkflowState::Ready,
            conversation_id: Some("conv-9".into()),
            init_session_id: None,
            workspace_rel: "apps/app00001/workspace".into(),
        };
        let record_dto = AppRecordDto {
            id: "app00001".into(),
            name: "Habits".into(),
            brief: "a habit tracker".into(),
            git_enabled: true,
            created_at_ms: 11,
            updated_at_ms: 22,
            workflow_state: AppWorkflowStateDto::Ready,
            conversation_id: Some("conv-9".into()),
            init_session_id: None,
            workspace_rel: "apps/app00001/workspace".into(),
        };
        let checkpoint = AppCheckpoint {
            id: "cp-1".into(),
            label: "scaffold".into(),
            kind: AppCheckpointKind::ScaffoldCreated,
            created_at_ms: 33,
        };

        let cases: Vec<(AppEvent, ClientEvent)> = vec![
            (
                AppEvent::AppsChanged {
                    apps: vec![record.clone()],
                },
                ClientEvent::AppsChanged {
                    apps: vec![record_dto],
                },
            ),
            (
                AppEvent::WorkflowChanged {
                    app_id: "app00001".into(),
                    state: AppWorkflowState::Ready,
                    detail: Some("built".into()),
                },
                ClientEvent::AppWorkflowChanged {
                    app_id: "app00001".into(),
                    state: AppWorkflowStateDto::Ready,
                    detail: Some("built".into()),
                },
            ),
            (
                AppEvent::RuntimeChanged {
                    app_id: "app00001".into(),
                    runtime: local_apps::AppRuntimeRecord {
                        schema_version: local_apps::APPS_SCHEMA_VERSION,
                        app_id: "app00001".into(),
                        state: AppRuntimeState::Failed,
                        mode: Some(local_apps::AppRuntimeMode::StaticExport),
                        port: Some(3100),
                        pid: None,
                        last_error: Some("port died".into()),
                        updated_at_ms: 1,
                    },
                },
                ClientEvent::AppRuntimeChanged {
                    app_id: "app00001".into(),
                    state: AppRuntimeStateDto::Failed,
                    details: Some(AppRuntimeDetailsDto {
                        state: AppRuntimeStateDto::Failed,
                        mode: Some(AppRuntimeModeDto::StaticExport),
                        loopback_url: Some("http://127.0.0.1:3100".into()),
                        suspension_reason: None,
                        recovery_state: Some(AppRuntimeRecoveryStateDto::NotNeeded),
                        last_error: Some("port died".into()),
                    }),
                    last_error: Some("port died".into()),
                },
            ),
            (
                AppEvent::CheckpointCreated {
                    app_id: "app00001".into(),
                    checkpoint: checkpoint.clone(),
                },
                ClientEvent::AppCheckpointCreated {
                    app_id: "app00001".into(),
                    checkpoint: AppCheckpointDto {
                        id: "cp-1".into(),
                        label: "scaffold".into(),
                        kind: AppCheckpointKindDto::ScaffoldCreated,
                        created_at_ms: 33,
                    },
                },
            ),
        ];
        // Every `AppEvent` variant appears exactly once above. That
        // completeness is hand-maintained: a genuinely NEW `AppEvent`
        // variant forces a compile error into `lower_app_event`'s match, but
        // that compile error does NOT, by itself, force a new pair into this
        // list. Whoever adds the next variant should add a case here too,
        // but the compiler will not make them.
        for (domain, expected) in cases {
            assert_eq!(lower_app_event(domain), Some(expected));
        }
    }
}
