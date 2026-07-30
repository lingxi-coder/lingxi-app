//! Local-apps domain ⇄ client-protocol bridge (phase 1, T3).
//!
//! The `local-apps` core deliberately knows nothing about the client protocol
//! (its module doc: "the engine maps `AppEvent`s onto client events and
//! `AppError`s onto `AppOperationFailed`"). This module is that mapping:
//!
//! - [`SinkAppEventObserver`] forwards every [`local_apps::AppEvent`] the
//!   engine-owned `AppService` emits onto the connection's single
//!   [`client_adapter::ClientEventSink`] (the same channel every other client
//!   event rides — governing decision §0.2), lowered 1:1 onto the `App*`
//!   [`ClientEvent`] variants;
//! - the `lower_*` helpers turn core types into their wire DTOs;
//! - the `raise_*` helpers turn inbound command DTOs back into core types.
//!   The DTO enums are `#[non_exhaustive]`, so raising is fallible: a variant
//!   this engine version does not know fails typed with
//!   [`AppError::InvalidRequest`] instead of being silently dropped.
//!
//! uniffi-gated like the `host` module — it names the client-protocol DTO
//! surface, which is pulled only under that feature.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use async_trait::async_trait;
use client_adapter::ClientEventSink;
use client_protocol::events::ClientEvent;
use client_protocol::local_apps::{
    AppCheckpointDto, AppCheckpointKindDto, AppDesignPatchDto, AppDesignPatchOpDto,
    AppErrorCodeDto, AppRecordDto, AppRuntimeStateDto, AppTemplateKindDto, AppWorkflowStateDto,
    DensityLevelDto, DesignValueDto,
};
use local_apps::{
    AppCheckpoint, AppCheckpointKind, AppDesignPatch, AppDesignPatchOp, AppError, AppErrorCode,
    AppEvent, AppEventObserver, AppRecord, AppRuntimeState, AppTemplateKind, AppWorkflowState,
    DensityLevel, DesignValue,
};

/// [`AppEventObserver`] that lowers every domain event onto the connection's
/// event sink. Installed on the engine-owned `AppService` at build time so
/// service-driven events (including the phase-3 generator's, later) ride the
/// same outbound channel as command replies.
pub(crate) struct SinkAppEventObserver {
    sink: Arc<dyn ClientEventSink>,
}

impl SinkAppEventObserver {
    /// Observer over the connection's single outbound event sink.
    pub(crate) fn new(sink: Arc<dyn ClientEventSink>) -> Self {
        Self { sink }
    }
}

#[async_trait]
impl AppEventObserver for SinkAppEventObserver {
    async fn on_event(&self, event: AppEvent) {
        self.sink.emit(lower_app_event(event)).await;
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
}
