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

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use async_trait::async_trait;
use client_adapter::ClientEventSink;
use client_protocol::events::ClientEvent;
use client_protocol::local_apps::{
    AppCheckpointDto, AppCheckpointKindDto, AppCreateOriginDto, AppDesignPatchDto,
    AppDesignPatchOpDto, AppErrorCodeDto, AppRecordDto, AppRuntimeStateDto, AppTemplateKindDto,
    AppWorkflowStateDto, DensityLevelDto, DesignValueDto,
};
use local_apps::{
    AppCheckpoint, AppCheckpointKind, AppDesignPatch, AppDesignPatchOp, AppError, AppErrorCode,
    AppEvent, AppEventObserver, AppRecord, AppRuntimeState, AppService, AppTemplateKind,
    AppWorkflowState, DensityLevel, DesignValue,
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
    pub(crate) fn spawn(
        runtime: &tokio::runtime::Handle,
        sink: Arc<dyn ClientEventSink>,
    ) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel();
        runtime.spawn(async move {
            while let Some(emission) = rx.recv().await {
                match emission {
                    AppEmission::Domain(event) => sink.emit(lower_app_event(event)).await,
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
    /// emission-order guard), so a failure can never overtake its own cause
    /// (e.g. `AppDesignConflict` before the `revision_conflict` failure).
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
/// so service-driven events (including the phase-3 generator's, later) ride
/// the same outbound channel as command replies.
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
pub(crate) fn lower_app_event(event: AppEvent) -> ClientEvent {
    match event {
        AppEvent::AppsChanged { apps } => ClientEvent::AppsChanged {
            apps: lower_records(&apps),
        },
        AppEvent::DesignerRequested {
            app_id,
            interaction_id,
            revision,
        } => ClientEvent::AppDesignerRequested {
            app_id,
            interaction_id,
            revision,
        },
        AppEvent::DesignDraftChanged {
            app_id,
            revision,
            fields,
        } => ClientEvent::AppDesignDraftChanged {
            app_id,
            revision,
            fields: lower_fields(fields),
        },
        AppEvent::DesignSuggestionAvailable {
            app_id,
            suggestion_id,
            based_on_revision,
            patch,
        } => ClientEvent::AppDesignSuggestionAvailable {
            app_id,
            suggestion_id,
            based_on_revision,
            patch: lower_patch(patch),
        },
        AppEvent::DesignConflict {
            app_id,
            expected_revision,
            actual_revision,
        } => ClientEvent::AppDesignConflict {
            app_id,
            expected_revision,
            actual_revision,
        },
        AppEvent::WorkflowChanged {
            app_id,
            state,
            detail,
        } => ClientEvent::AppWorkflowChanged {
            app_id,
            state: lower_workflow_state(state),
            detail,
        },
        AppEvent::GenerationProgress(progress) => ClientEvent::AppGenerationProgress {
            app_id: progress.app_id,
            stage: progress.stage,
            percent: progress.percent,
            detail: progress.detail,
        },
        AppEvent::RuntimeChanged {
            app_id,
            state,
            last_error,
        } => ClientEvent::AppRuntimeChanged {
            app_id,
            state: lower_runtime_state(state),
            last_error,
        },
        AppEvent::PreviewReady {
            app_id,
            interaction_id,
            revision,
            url,
        } => ClientEvent::AppPreviewReady {
            app_id,
            interaction_id,
            revision,
            url,
        },
        AppEvent::CheckpointCreated { app_id, checkpoint } => ClientEvent::AppCheckpointCreated {
            app_id,
            checkpoint: lower_checkpoint(&checkpoint),
        },
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
        template: lower_template(record.template),
        created_at_ms: record.created_at_ms,
        updated_at_ms: record.updated_at_ms,
        workflow_state: lower_workflow_state(record.workflow_state),
        conversation_id: record.conversation_id.clone(),
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
    }
}

fn lower_template(template: AppTemplateKind) -> AppTemplateKindDto {
    match template {
        AppTemplateKind::Dashboard => AppTemplateKindDto::Dashboard,
        AppTemplateKind::CrudTracker => AppTemplateKindDto::CrudTracker,
        AppTemplateKind::ContentShowcase => AppTemplateKindDto::ContentShowcase,
        AppTemplateKind::FormUtility => AppTemplateKindDto::FormUtility,
    }
}

fn lower_workflow_state(state: AppWorkflowState) -> AppWorkflowStateDto {
    match state {
        AppWorkflowState::CollectingSpec => AppWorkflowStateDto::CollectingSpec,
        AppWorkflowState::AwaitingSpecConfirmation => AppWorkflowStateDto::AwaitingSpecConfirmation,
        AppWorkflowState::Generating => AppWorkflowStateDto::Generating,
        AppWorkflowState::Validating => AppWorkflowStateDto::Validating,
        AppWorkflowState::AwaitingPreviewConfirmation => {
            AppWorkflowStateDto::AwaitingPreviewConfirmation
        }
        AppWorkflowState::Revising => AppWorkflowStateDto::Revising,
        AppWorkflowState::Ready => AppWorkflowStateDto::Ready,
        AppWorkflowState::GenerationFailed => AppWorkflowStateDto::GenerationFailed,
        AppWorkflowState::ValidationFailed => AppWorkflowStateDto::ValidationFailed,
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

fn lower_density(level: DensityLevel) -> DensityLevelDto {
    match level {
        DensityLevel::Compact => DensityLevelDto::Compact,
        DensityLevel::Comfortable => DensityLevelDto::Comfortable,
    }
}

fn lower_design_value(value: DesignValue) -> DesignValueDto {
    match value {
        DesignValue::ShortText(value) => DesignValueDto::ShortText { value },
        DesignValue::LongText(value) => DesignValueDto::LongText { value },
        DesignValue::SingleChoice(value) => DesignValueDto::SingleChoice { value },
        DesignValue::MultipleChoice(value) => DesignValueDto::MultipleChoice { value },
        DesignValue::Boolean(value) => DesignValueDto::Boolean { value },
        DesignValue::Color(value) => DesignValueDto::Color { value },
        DesignValue::Density(level) => DesignValueDto::Density {
            value: lower_density(level),
        },
        DesignValue::ScreenList(value) => DesignValueDto::ScreenList { value },
        DesignValue::FeatureList(value) => DesignValueDto::FeatureList { value },
    }
}

/// Lower a full draft field map for `AppDesignDraftChanged`.
fn lower_fields(fields: BTreeMap<String, DesignValue>) -> HashMap<String, DesignValueDto> {
    fields
        .into_iter()
        .map(|(field_id, value)| (field_id, lower_design_value(value)))
        .collect()
}

/// Lower a core patch for `AppDesignSuggestionAvailable`.
pub(crate) fn lower_patch(patch: AppDesignPatch) -> AppDesignPatchDto {
    AppDesignPatchDto {
        ops: patch
            .ops
            .into_iter()
            .map(|op| match op {
                AppDesignPatchOp::Set { field_id, value } => AppDesignPatchOpDto::Set {
                    field_id,
                    value: lower_design_value(value),
                },
                AppDesignPatchOp::Remove { field_id } => AppDesignPatchOpDto::Remove { field_id },
            })
            .collect(),
        note: patch.note,
    }
}

/// Raise an inbound template DTO to the core kind.
///
/// # Errors
///
/// `invalid_request` for a `#[non_exhaustive]` template this engine version
/// does not know.
pub(crate) fn raise_template(template: AppTemplateKindDto) -> Result<AppTemplateKind, AppError> {
    Ok(match template {
        AppTemplateKindDto::Dashboard => AppTemplateKind::Dashboard,
        AppTemplateKindDto::CrudTracker => AppTemplateKind::CrudTracker,
        AppTemplateKindDto::ContentShowcase => AppTemplateKind::ContentShowcase,
        AppTemplateKindDto::FormUtility => AppTemplateKind::FormUtility,
        other => {
            return Err(AppError::InvalidRequest(format!(
                "unsupported app template: {other:?}"
            )))
        }
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

/// Raise an inbound patch DTO to the core patch.
///
/// # Errors
///
/// `invalid_request` for a `#[non_exhaustive]` op / value / density variant
/// this engine version does not know.
pub(crate) fn raise_patch(patch: AppDesignPatchDto) -> Result<AppDesignPatch, AppError> {
    let mut ops = Vec::with_capacity(patch.ops.len());
    for op in patch.ops {
        ops.push(match op {
            AppDesignPatchOpDto::Set { field_id, value } => AppDesignPatchOp::Set {
                field_id,
                value: raise_design_value(value)?,
            },
            AppDesignPatchOpDto::Remove { field_id } => AppDesignPatchOp::Remove { field_id },
            other => {
                return Err(AppError::InvalidRequest(format!(
                    "unsupported design patch op: {other:?}"
                )))
            }
        });
    }
    Ok(AppDesignPatch {
        ops,
        note: patch.note,
    })
}

fn raise_design_value(value: DesignValueDto) -> Result<DesignValue, AppError> {
    Ok(match value {
        DesignValueDto::ShortText { value } => DesignValue::ShortText(value),
        DesignValueDto::LongText { value } => DesignValue::LongText(value),
        DesignValueDto::SingleChoice { value } => DesignValue::SingleChoice(value),
        DesignValueDto::MultipleChoice { value } => DesignValue::MultipleChoice(value),
        DesignValueDto::Boolean { value } => DesignValue::Boolean(value),
        DesignValueDto::Color { value } => DesignValue::Color(value),
        DesignValueDto::Density { value } => DesignValue::Density(raise_density(value)?),
        DesignValueDto::ScreenList { value } => DesignValue::ScreenList(value),
        DesignValueDto::FeatureList { value } => DesignValue::FeatureList(value),
        other => {
            return Err(AppError::InvalidRequest(format!(
                "unsupported design value: {other:?}"
            )))
        }
    })
}

fn raise_density(level: DensityLevelDto) -> Result<DensityLevel, AppError> {
    match level {
        DensityLevelDto::Compact => Ok(DensityLevel::Compact),
        DensityLevelDto::Comfortable => Ok(DensityLevel::Comfortable),
        other => Err(AppError::InvalidRequest(format!(
            "unsupported density level: {other:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use local_apps::AppWorkflowState;

    fn every_design_value() -> Vec<DesignValue> {
        vec![
            DesignValue::ShortText("t".into()),
            DesignValue::LongText("body".into()),
            DesignValue::SingleChoice("a".into()),
            DesignValue::MultipleChoice(vec!["a".into(), "b".into()]),
            DesignValue::Boolean(true),
            DesignValue::Color("#aabbcc".into()),
            DesignValue::Density(DensityLevel::Compact),
            DesignValue::ScreenList(vec!["home".into()]),
            DesignValue::FeatureList(vec!["export".into()]),
        ]
    }

    #[test]
    fn patch_round_trips_through_dto_for_every_value_kind() {
        let patch = AppDesignPatch {
            ops: every_design_value()
                .into_iter()
                .enumerate()
                .map(|(i, value)| AppDesignPatchOp::Set {
                    field_id: format!("f{i}"),
                    value,
                })
                .chain(std::iter::once(AppDesignPatchOp::Remove {
                    field_id: "gone".into(),
                }))
                .collect(),
            note: Some("polish".into()),
        };
        let raised = raise_patch(lower_patch(patch.clone())).expect("round trip");
        assert_eq!(raised, patch);
    }

    #[test]
    fn dto_and_core_agree_on_the_wire_strings() {
        // The DTOs mirror the core enums byte-for-byte on the wire; comparing
        // serde output guards against either side drifting.
        for (core, dto) in [
            (
                serde_json::to_string(&AppTemplateKind::CrudTracker).unwrap(),
                serde_json::to_string(&lower_template(AppTemplateKind::CrudTracker)).unwrap(),
            ),
            (
                serde_json::to_string(&AppWorkflowState::AwaitingPreviewConfirmation).unwrap(),
                serde_json::to_string(&lower_workflow_state(
                    AppWorkflowState::AwaitingPreviewConfirmation,
                ))
                .unwrap(),
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
            state: AppWorkflowState::Generating,
            detail: Some("confirmed".into()),
        });
        match event {
            ClientEvent::AppWorkflowChanged {
                app_id,
                state,
                detail,
            } => {
                assert_eq!(app_id, "abcd1234");
                assert_eq!(state, AppWorkflowStateDto::Generating);
                assert_eq!(detail.as_deref(), Some("confirmed"));
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
        let library =
            raise_origin(AppCreateOriginDto::Library).expect("library is a known origin");
        assert_eq!(library, AppCreateOrigin::Library);
        assert_eq!(library.conversation_binding(Some("conv-1".into())), None);
        // The failing wildcard arm cannot be exercised from this crate:
        // `AppCreateOriginDto` is `#[non_exhaustive]` in `client-protocol`,
        // and every variant that exists today is matched above — the arm
        // exists precisely for variants a FUTURE client-protocol version
        // adds, which must fail typed (`invalid_request`) instead of
        // laundering into a library create.
    }

    /// W4: exact field mapping for EVERY `lower_app_event` arm (one pair per
    /// `AppEvent` variant). The expectation side uses exhaustive struct
    /// literals with a distinct value per field, so a transposed or dropped
    /// field fails here instead of shipping silently.
    // One pair per variant is the point of the test; splitting it would lose
    // the at-a-glance completeness of the arm list.
    #[allow(clippy::too_many_lines)]
    #[test]
    fn every_app_event_arm_lowers_field_exact() {
        use local_apps::AppGenerationProgress;

        let record = AppRecord {
            id: "app00001".into(),
            name: "Habits".into(),
            template: AppTemplateKind::CrudTracker,
            created_at_ms: 11,
            updated_at_ms: 22,
            workflow_state: AppWorkflowState::Ready,
            conversation_id: Some("conv-9".into()),
            workspace_rel: "apps/app00001/workspace".into(),
        };
        let record_dto = AppRecordDto {
            id: "app00001".into(),
            name: "Habits".into(),
            template: AppTemplateKindDto::CrudTracker,
            created_at_ms: 11,
            updated_at_ms: 22,
            workflow_state: AppWorkflowStateDto::Ready,
            conversation_id: Some("conv-9".into()),
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
                AppEvent::DesignerRequested {
                    app_id: "app00001".into(),
                    interaction_id: "int-1".into(),
                    revision: 4,
                },
                ClientEvent::AppDesignerRequested {
                    app_id: "app00001".into(),
                    interaction_id: "int-1".into(),
                    revision: 4,
                },
            ),
            (
                AppEvent::DesignDraftChanged {
                    app_id: "app00001".into(),
                    revision: 5,
                    fields: BTreeMap::from([(
                        "title".to_string(),
                        DesignValue::ShortText("Mine".into()),
                    )]),
                },
                ClientEvent::AppDesignDraftChanged {
                    app_id: "app00001".into(),
                    revision: 5,
                    fields: HashMap::from([(
                        "title".to_string(),
                        DesignValueDto::ShortText {
                            value: "Mine".into(),
                        },
                    )]),
                },
            ),
            (
                AppEvent::DesignSuggestionAvailable {
                    app_id: "app00001".into(),
                    suggestion_id: "sugg-1".into(),
                    based_on_revision: 6,
                    patch: AppDesignPatch {
                        ops: vec![AppDesignPatchOp::Remove {
                            field_id: "gone".into(),
                        }],
                        note: Some("polish".into()),
                    },
                },
                ClientEvent::AppDesignSuggestionAvailable {
                    app_id: "app00001".into(),
                    suggestion_id: "sugg-1".into(),
                    based_on_revision: 6,
                    patch: AppDesignPatchDto {
                        ops: vec![AppDesignPatchOpDto::Remove {
                            field_id: "gone".into(),
                        }],
                        note: Some("polish".into()),
                    },
                },
            ),
            (
                AppEvent::DesignConflict {
                    app_id: "app00001".into(),
                    expected_revision: 7,
                    actual_revision: 8,
                },
                ClientEvent::AppDesignConflict {
                    app_id: "app00001".into(),
                    expected_revision: 7,
                    actual_revision: 8,
                },
            ),
            (
                AppEvent::WorkflowChanged {
                    app_id: "app00001".into(),
                    state: AppWorkflowState::Validating,
                    detail: Some("checking".into()),
                },
                ClientEvent::AppWorkflowChanged {
                    app_id: "app00001".into(),
                    state: AppWorkflowStateDto::Validating,
                    detail: Some("checking".into()),
                },
            ),
            (
                AppEvent::GenerationProgress(AppGenerationProgress {
                    app_id: "app00001".into(),
                    stage: "pages".into(),
                    percent: Some(42),
                    detail: Some("3/7".into()),
                }),
                ClientEvent::AppGenerationProgress {
                    app_id: "app00001".into(),
                    stage: "pages".into(),
                    percent: Some(42),
                    detail: Some("3/7".into()),
                },
            ),
            (
                AppEvent::RuntimeChanged {
                    app_id: "app00001".into(),
                    state: AppRuntimeState::Failed,
                    last_error: Some("port died".into()),
                },
                ClientEvent::AppRuntimeChanged {
                    app_id: "app00001".into(),
                    state: AppRuntimeStateDto::Failed,
                    last_error: Some("port died".into()),
                },
            ),
            (
                AppEvent::PreviewReady {
                    app_id: "app00001".into(),
                    interaction_id: "int-2".into(),
                    revision: 9,
                    url: Some("http://127.0.0.1:3001".into()),
                },
                ClientEvent::AppPreviewReady {
                    app_id: "app00001".into(),
                    interaction_id: "int-2".into(),
                    revision: 9,
                    url: Some("http://127.0.0.1:3001".into()),
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
        // Every `AppEvent` variant appears exactly once above; a new variant
        // extends `lower_app_event`'s match (compile error) and belongs here.
        for (domain, expected) in cases {
            assert_eq!(lower_app_event(domain), expected);
        }
    }
}
