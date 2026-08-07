//! Pure in-memory aggregate for one app plus both state machines (spec §B/§C).
//!
//! Every transition is an explicit method returning a typed [`AppError`];
//! anything outside the exhaustive transition table fails with
//! `workflow_state_invalid` (or `invalid_request` for the runtime machine).
//! The aggregate performs NO I/O — [`crate::service::AppService`] owns
//! persistence and event emission around these methods.

use crate::error::AppError;
use crate::storage;
use crate::types::{
    AppContinuation, AppContinuationKind, AppDesignDraft, AppDesignPatch, AppDesignPatchOp,
    AppDesignSuggestion, AppInteractionKind, AppInteractionRequest, AppInteractions, AppRecord,
    AppRuntimeMode, AppRuntimeRecord, AppRuntimeState, AppWorkflowState, APPS_SCHEMA_VERSION,
};
use std::collections::BTreeMap;

/// Workflow states in which the draft may be edited (`update_draft`,
/// `store_suggestion`, `apply_suggestion`).
pub const DRAFT_EDITABLE_STATES: [AppWorkflowState; 3] = [
    AppWorkflowState::CollectingSpec,
    AppWorkflowState::AwaitingSpecConfirmation,
    AppWorkflowState::Revising,
];

/// Secondary count bound on the per-app undelivered queue. Delivery is
/// at-least-once (spec §E) — but only while the store itself stays loadable:
/// with a persistently dead sink an unbounded queue would grow
/// `interactions.json` without limit and eventually past
/// [`crate::storage::MAX_DOC_BYTES`], bricking the whole store at the next
/// load. TRADEOFF, chosen deliberately: on overflow the OLDEST undelivered
/// continuation is dropped (with a warning) to admit the newest — store
/// survival beats perfect at-least-once delivery when the sink is
/// persistently dead. Consumers already dedup by seq, so a dropped seq is
/// simply a gate outcome the conversation never learns about.
///
/// The count alone cannot protect the store (finding: 256 legal 20 KB
/// prompts serialize far past the document bound), so
/// [`MAX_UNDELIVERED_BYTES`] is the PRIMARY cap; this count stays as a
/// secondary bound for pathological many-tiny-entry queues.
pub const MAX_UNDELIVERED_CONTINUATIONS: usize = 256;

/// PRIMARY byte budget for the undelivered queue: the sum of each entry's
/// serialized (compact JSON, escaping included) size must stay at or below
/// 4 MiB. Why 4 MiB: half of [`crate::storage::MAX_DOC_BYTES`] (8 MiB)
/// leaves headroom for the rest of `interactions.json` (pending gate,
/// counters), pretty-print indentation, and the approximation error of
/// summing compact per-entry sizes — a queue at this budget can always be
/// persisted, so legal max-size prompts can never wedge a gate op behind the
/// document size limit.
pub const MAX_UNDELIVERED_BYTES: usize = 4 * 1024 * 1024;

/// Approximate serialized size of one queued continuation (compact JSON —
/// escaping counted, pretty-print indentation not; see
/// [`MAX_UNDELIVERED_BYTES`] for the headroom that absorbs the difference).
/// Serialization failure is impossible for values the store accepted
/// (`write_doc` serializes the same data); 0 keeps the cap fail-open rather
/// than panicking on an unrepresentable entry.
fn approx_continuation_bytes(continuation: &AppContinuation) -> usize {
    serde_json::to_string(continuation).map_or(0, |body| body.len())
}

/// Drop oldest undelivered continuations (warning per drop) until the queue
/// fits BOTH caps: [`MAX_UNDELIVERED_BYTES`] (primary, byte-aware — sizes
/// are computed once per call and the running total is adjusted
/// incrementally as entries drop) and [`MAX_UNDELIVERED_CONTINUATIONS`]
/// (secondary count bound). Shared by the mint path ([`AppState`]'s enqueue)
/// and the service's redelivery merge path so the caps hold everywhere the
/// queue is (re)built.
pub(crate) fn enforce_undelivered_cap(app_id: &str, interactions: &mut AppInteractions) {
    let mut sizes: Vec<usize> = interactions
        .undelivered
        .iter()
        .map(approx_continuation_bytes)
        .collect();
    let mut total_bytes: usize = sizes.iter().sum();
    while !interactions.undelivered.is_empty()
        && (interactions.undelivered.len() > MAX_UNDELIVERED_CONTINUATIONS
            || total_bytes > MAX_UNDELIVERED_BYTES)
    {
        let dropped = interactions.undelivered.remove(0);
        total_bytes -= sizes.remove(0);
        tracing::warn!(
            app_id,
            dropped_seq = dropped.seq,
            dropped_kind = %dropped.kind,
            queued = interactions.undelivered.len(),
            queued_bytes = total_bytes,
            "undelivered continuation queue overflowed; dropped the oldest \
             (store survival beats perfect at-least-once with a dead sink)"
        );
    }
}

/// True iff `from -> to` is one of the legal runtime transitions:
/// `stopped -> starting -> running -> stopping -> stopped`,
/// `starting -> failed`, `running -> failed`, `failed -> starting`.
#[must_use]
pub fn runtime_transition_allowed(from: AppRuntimeState, to: AppRuntimeState) -> bool {
    use AppRuntimeState::{Failed, Running, Starting, Stopped, Stopping};
    matches!(
        (from, to),
        (Stopped | Failed, Starting)
            | (Starting, Running)
            | (Running, Stopping)
            | (Stopping, Stopped)
            | (Starting | Running, Failed)
    )
}

/// The full in-memory state of one app: record + draft + interaction store +
/// runtime record (mirroring the four persisted documents).
#[derive(Debug, Clone, PartialEq)]
pub struct AppState {
    /// Index/mirror record.
    pub record: AppRecord,
    /// Design draft (`workspace/.lingxi/design-spec.json`).
    pub draft: AppDesignDraft,
    /// Pending gate + continuation queue (`interactions.json`).
    pub interactions: AppInteractions,
    /// Runtime record (`runtime.json`).
    pub runtime: AppRuntimeRecord,
}

impl AppState {
    /// Build the aggregate for a brand-new app in `collecting_spec`.
    #[must_use]
    pub fn create(
        id: String,
        name: String,
        brief: String,
        conversation_id: Option<String>,
        now_ms: u64,
    ) -> Self {
        let workspace_rel = storage::workspace_rel_str(&id);
        Self {
            record: AppRecord {
                id: id.clone(),
                name,
                brief,
                created_at_ms: now_ms,
                updated_at_ms: now_ms,
                workflow_state: AppWorkflowState::AuthoringQuestionnaire,
                conversation_id,
                workspace_rel,
            },
            draft: AppDesignDraft {
                schema_version: APPS_SCHEMA_VERSION,
                revision: 0,
                questionnaire: Vec::new(),
                fields: BTreeMap::new(),
                plan: None,
                plan_for_revision: None,
                pending_suggestion: None,
                confirmed_revision: None,
            },
            interactions: AppInteractions::new(),
            runtime: AppRuntimeRecord {
                schema_version: APPS_SCHEMA_VERSION,
                app_id: id,
                state: AppRuntimeState::Stopped,
                mode: None,
                port: None,
                pid: None,
                last_error: None,
                updated_at_ms: now_ms,
            },
        }
    }

    fn ensure_workflow(&self, op: &str, allowed: &[AppWorkflowState]) -> Result<(), AppError> {
        let current = self.record.workflow_state;
        if allowed.contains(&current) {
            Ok(())
        } else {
            Err(AppError::WorkflowStateInvalid(format!(
                "{op} is not allowed while app {} is in workflow state {current}",
                self.record.id
            )))
        }
    }

    fn ensure_current_revision(&self, expected: u64) -> Result<(), AppError> {
        if expected == self.draft.revision {
            Ok(())
        } else {
            Err(AppError::RevisionConflict {
                expected,
                actual: self.draft.revision,
            })
        }
    }

    fn validate_pending(
        &self,
        op: &str,
        kind: AppInteractionKind,
        interaction_id: &str,
    ) -> Result<(), AppError> {
        match &self.interactions.pending {
            Some(pending) if pending.kind == kind && pending.interaction_id == interaction_id => {
                Ok(())
            }
            Some(_) => Err(AppError::InteractionInvalid(format!(
                "{op}: {interaction_id:?} is not the pending {kind} interaction for app {}",
                self.record.id
            ))),
            None => Err(AppError::InteractionInvalid(format!(
                "{op}: app {} has no pending interaction",
                self.record.id
            ))),
        }
    }

    fn set_workflow(&mut self, next: AppWorkflowState, now_ms: u64) {
        self.record.workflow_state = next;
        self.record.updated_at_ms = now_ms;
    }

    fn apply_patch(&mut self, patch: &AppDesignPatch) {
        for op in &patch.ops {
            match op {
                AppDesignPatchOp::Set { field_id, value } => {
                    self.draft.fields.insert(field_id.clone(), value.clone());
                }
                AppDesignPatchOp::Remove { field_id } => {
                    self.draft.fields.remove(field_id);
                }
            }
        }
    }

    fn enqueue_continuation(
        &mut self,
        kind: AppContinuationKind,
        payload: serde_json::Value,
        now_ms: u64,
    ) -> AppContinuation {
        let continuation = AppContinuation {
            seq: self.interactions.next_seq,
            app_id: self.record.id.clone(),
            kind,
            payload,
            created_at_ms: now_ms,
        };
        self.interactions.next_seq += 1;
        self.interactions.undelivered.push(continuation.clone());
        enforce_undelivered_cap(&self.record.id, &mut self.interactions);
        continuation
    }

    /// `authoring_questionnaire -> collecting_spec`，落盘问卷；`name` 是
    /// LLM 建议的正式名，用来替换创建时的占位名。
    pub fn questionnaire_ready(
        &mut self,
        steps: Vec<crate::questionnaire::AppDesignStep>,
        name: Option<String>,
        now_ms: u64,
    ) -> Result<(), AppError> {
        self.ensure_workflow(
            "questionnaire_ready",
            &[AppWorkflowState::AuthoringQuestionnaire],
        )?;
        crate::questionnaire::validate_questionnaire(&steps)?;
        self.draft.questionnaire = steps;
        if let Some(name) = name {
            let trimmed = name.trim();
            if !trimmed.is_empty() {
                self.record.name = trimmed.to_string();
            }
        }
        self.set_workflow(AppWorkflowState::CollectingSpec, now_ms);
        Ok(())
    }

    /// `authoring_questionnaire -> questionnaire_failed`.
    pub fn questionnaire_failed(&mut self, now_ms: u64) -> Result<(), AppError> {
        self.ensure_workflow(
            "questionnaire_failed",
            &[AppWorkflowState::AuthoringQuestionnaire],
        )?;
        self.set_workflow(AppWorkflowState::QuestionnaireFailed, now_ms);
        Ok(())
    }

    /// `questionnaire_failed -> authoring_questionnaire`. 重试回到执行态，
    /// 不是跳过它。
    pub fn retry_questionnaire(&mut self, now_ms: u64) -> Result<(), AppError> {
        self.ensure_workflow(
            "retry_questionnaire",
            &[AppWorkflowState::QuestionnaireFailed],
        )?;
        self.set_workflow(AppWorkflowState::AuthoringQuestionnaire, now_ms);
        Ok(())
    }

    /// 改 brief 并重新出题。旧答案的 field id 在新问卷里已不存在，
    /// 保留它们只会让后续校验对着幽灵字段报错——一并清掉。
    pub fn update_brief(&mut self, brief: String, now_ms: u64) -> Result<(), AppError> {
        self.ensure_workflow(
            "update_brief",
            &[
                AppWorkflowState::CollectingSpec,
                AppWorkflowState::QuestionnaireFailed,
            ],
        )?;
        let trimmed = brief.trim();
        if trimmed.is_empty() {
            return Err(AppError::InvalidRequest("brief is empty".into()));
        }
        self.record.brief = trimmed.to_string();
        self.draft.questionnaire.clear();
        self.draft.fields.clear();
        self.draft.plan = None;
        self.draft.plan_for_revision = None;
        self.draft.pending_suggestion = None;
        self.draft.revision += 1;
        self.set_workflow(AppWorkflowState::AuthoringQuestionnaire, now_ms);
        Ok(())
    }

    /// `collecting_spec -> planning`。先确认答案自洽，别拿一份残缺答案
    /// 去换一次 LLM 往返。
    pub fn begin_planning(&mut self, now_ms: u64) -> Result<(), AppError> {
        self.ensure_workflow("begin_planning", &[AppWorkflowState::CollectingSpec])?;
        crate::questionnaire::validate_answers(&self.draft.questionnaire, &self.draft.fields)?;
        self.set_workflow(AppWorkflowState::Planning, now_ms);
        Ok(())
    }

    /// `planning -> awaiting_spec_confirmation`，落盘方案并开确认门。
    pub fn plan_ready(
        &mut self,
        plan: crate::questionnaire::AppPlan,
        interaction_id: String,
        now_ms: u64,
    ) -> Result<AppInteractionRequest, AppError> {
        self.ensure_workflow("plan_ready", &[AppWorkflowState::Planning])?;
        crate::questionnaire::validate_plan(&plan)?;
        self.draft.plan = Some(plan);
        self.draft.plan_for_revision = Some(self.draft.revision);
        let interaction = AppInteractionRequest {
            interaction_id,
            app_id: self.record.id.clone(),
            kind: AppInteractionKind::Designer,
            revision: self.draft.revision,
            created_at_ms: now_ms,
        };
        self.interactions.pending = Some(interaction.clone());
        self.set_workflow(AppWorkflowState::AwaitingSpecConfirmation, now_ms);
        Ok(interaction)
    }

    /// `planning -> plan_failed`.
    pub fn plan_failed(&mut self, now_ms: u64) -> Result<(), AppError> {
        self.ensure_workflow("plan_failed", &[AppWorkflowState::Planning])?;
        self.set_workflow(AppWorkflowState::PlanFailed, now_ms);
        Ok(())
    }

    /// `plan_failed -> planning`.
    pub fn retry_plan(&mut self, now_ms: u64) -> Result<(), AppError> {
        self.ensure_workflow("retry_plan", &[AppWorkflowState::PlanFailed])?;
        self.set_workflow(AppWorkflowState::Planning, now_ms);
        Ok(())
    }

    /// `collecting_spec | generation_failed -> awaiting_spec_confirmation`;
    /// opens the single pending designer interaction at the current revision.
    ///
    /// `generation_failed` is a legal source because generation usually fails
    /// on the DESIGN (an id or domain the manifest contract rejects) and
    /// [`Self::retry_generation`] can only replay the same confirmed revision
    /// — so re-opening the designer is what returns the draft to an editable
    /// state, the role [`Self::begin_revision`] plays for `validation_failed`.
    pub fn open_designer(
        &mut self,
        interaction_id: String,
        now_ms: u64,
    ) -> Result<AppInteractionRequest, AppError> {
        self.ensure_workflow(
            "open_designer",
            &[
                AppWorkflowState::CollectingSpec,
                AppWorkflowState::GenerationFailed,
            ],
        )?;
        let interaction = AppInteractionRequest {
            interaction_id,
            app_id: self.record.id.clone(),
            kind: AppInteractionKind::Designer,
            revision: self.draft.revision,
            created_at_ms: now_ms,
        };
        self.interactions.pending = Some(interaction.clone());
        self.set_workflow(AppWorkflowState::AwaitingSpecConfirmation, now_ms);
        Ok(interaction)
    }

    /// Apply a user edit at `expected_revision`. State is unchanged; the
    /// revision bumps by one. A stale `expected_revision` fails with
    /// `revision_conflict` and leaves the draft untouched (no silent
    /// overwrite). A pending designer interaction survives the edit.
    pub fn update_draft(
        &mut self,
        expected_revision: u64,
        patch: &AppDesignPatch,
        now_ms: u64,
    ) -> Result<u64, AppError> {
        self.ensure_workflow("update_draft", &DRAFT_EDITABLE_STATES)?;
        self.ensure_current_revision(expected_revision)?;
        self.apply_patch(patch);
        self.draft.revision += 1;
        // 答案变了，方案就不再是对这份答案的方案。
        self.draft.plan = None;
        self.draft.plan_for_revision = None;
        self.record.updated_at_ms = now_ms;
        Ok(self.draft.revision)
    }

    /// Store an agent suggestion for later explicit application. Does NOT
    /// touch fields or the revision; replaces any previously stored
    /// suggestion.
    pub fn store_suggestion(
        &mut self,
        suggestion_id: String,
        patch: AppDesignPatch,
        now_ms: u64,
    ) -> Result<AppDesignSuggestion, AppError> {
        self.ensure_workflow("store_suggestion", &DRAFT_EDITABLE_STATES)?;
        let suggestion = AppDesignSuggestion {
            suggestion_id,
            patch,
            based_on_revision: self.draft.revision,
        };
        self.draft.pending_suggestion = Some(suggestion.clone());
        self.record.updated_at_ms = now_ms;
        Ok(suggestion)
    }

    /// Consume the addressed pending suggestion without changing fields or
    /// the draft revision. A mismatched id leaves the suggestion intact.
    pub fn dismiss_suggestion(&mut self, suggestion_id: &str, now_ms: u64) -> Result<(), AppError> {
        self.ensure_workflow("dismiss_suggestion", &DRAFT_EDITABLE_STATES)?;
        match self.draft.pending_suggestion.take() {
            Some(pending) if pending.suggestion_id == suggestion_id => {
                self.record.updated_at_ms = now_ms;
                Ok(())
            }
            Some(pending) => {
                let pending_id = pending.suggestion_id.clone();
                self.draft.pending_suggestion = Some(pending);
                Err(AppError::InteractionInvalid(format!(
                    "dismiss_suggestion: {suggestion_id:?} does not match the pending suggestion {pending_id:?} for app {}",
                    self.record.id
                )))
            }
            None => Err(AppError::InteractionInvalid(format!(
                "dismiss_suggestion: app {} has no pending suggestion",
                self.record.id
            ))),
        }
    }

    /// Apply the stored suggestion. Same state/revision gating as
    /// [`Self::update_draft`]; a wrong `suggestion_id` fails with
    /// `interaction_invalid` (the suggestion survives).
    ///
    /// `based_on_revision` is LOAD-BEARING: the addressed suggestion is
    /// applied only when its `based_on_revision` equals the CURRENT draft
    /// revision — a stale suggestion silently overwriting newer user edits is
    /// exactly the lost-update `update_draft`'s revision gate exists to
    /// prevent. A stale suggestion is CONSUMED (revisions only grow, so it
    /// can never become applicable — left pending it would wedge the draft)
    /// and fails typed `revision_conflict { expected: based_on, actual:
    /// current }`; the service persists the consumption and emits
    /// `DesignConflict`.
    pub fn apply_suggestion(
        &mut self,
        suggestion_id: &str,
        expected_revision: u64,
        now_ms: u64,
    ) -> Result<u64, AppError> {
        self.ensure_workflow("apply_suggestion", &DRAFT_EDITABLE_STATES)?;
        self.ensure_current_revision(expected_revision)?;
        // Consume only when every remaining gate passes; restore on id
        // mismatch. (The staleness gate below deliberately KEEPS the take —
        // stale suggestions are consumed.)
        let suggestion = match self.draft.pending_suggestion.take() {
            Some(pending) if pending.suggestion_id == suggestion_id => pending,
            Some(pending) => {
                let pending_id = pending.suggestion_id.clone();
                self.draft.pending_suggestion = Some(pending);
                return Err(AppError::InteractionInvalid(format!(
                    "apply_suggestion: {suggestion_id:?} does not match the pending suggestion {pending_id:?} for app {}",
                    self.record.id
                )));
            }
            None => {
                return Err(AppError::InteractionInvalid(format!(
                    "apply_suggestion: app {} has no pending suggestion",
                    self.record.id
                )));
            }
        };
        if suggestion.based_on_revision != self.draft.revision {
            // Stale: consumed (not restored), fields and revision untouched.
            self.record.updated_at_ms = now_ms;
            return Err(AppError::RevisionConflict {
                expected: suggestion.based_on_revision,
                actual: self.draft.revision,
            });
        }
        self.apply_patch(&suggestion.patch);
        self.draft.revision += 1;
        self.record.updated_at_ms = now_ms;
        Ok(self.draft.revision)
    }

    /// `awaiting_spec_confirmation -> generating`. Requires the exact pending
    /// designer `interaction_id` AND the current draft revision; records the
    /// confirmed revision, consumes the gate, and enqueues a
    /// `design_confirmed` continuation.
    pub fn confirm_design(
        &mut self,
        interaction_id: &str,
        revision: u64,
        now_ms: u64,
    ) -> Result<AppContinuation, AppError> {
        self.ensure_workflow(
            "confirm_design",
            &[AppWorkflowState::AwaitingSpecConfirmation],
        )?;
        self.validate_pending(
            "confirm_design",
            AppInteractionKind::Designer,
            interaction_id,
        )?;
        self.ensure_current_revision(revision)?;
        if self.draft.plan_for_revision != Some(self.draft.revision) {
            return Err(AppError::RevisionConflict {
                expected: self.draft.plan_for_revision.unwrap_or_default(),
                actual: self.draft.revision,
            });
        }
        self.interactions.pending = None;
        self.draft.confirmed_revision = Some(revision);
        self.set_workflow(AppWorkflowState::Generating, now_ms);
        Ok(self.enqueue_continuation(
            AppContinuationKind::DesignConfirmed,
            serde_json::json!({ "revision": revision }),
            now_ms,
        ))
    }

    /// `awaiting_spec_confirmation -> collecting_spec`; voids the pending
    /// interaction and enqueues a `design_cancelled` continuation.
    pub fn cancel_design(&mut self, now_ms: u64) -> Result<AppContinuation, AppError> {
        self.ensure_workflow(
            "cancel_design",
            &[AppWorkflowState::AwaitingSpecConfirmation],
        )?;
        self.interactions.pending = None;
        self.set_workflow(AppWorkflowState::CollectingSpec, now_ms);
        Ok(self.enqueue_continuation(
            AppContinuationKind::DesignCancelled,
            serde_json::json!({}),
            now_ms,
        ))
    }

    /// `generating -> validating`.
    pub fn generation_complete(&mut self, now_ms: u64) -> Result<(), AppError> {
        self.ensure_workflow("generation_complete", &[AppWorkflowState::Generating])?;
        self.set_workflow(AppWorkflowState::Validating, now_ms);
        Ok(())
    }

    /// `generating -> generation_failed`.
    pub fn generation_failed(&mut self, now_ms: u64) -> Result<(), AppError> {
        self.ensure_workflow("generation_failed", &[AppWorkflowState::Generating])?;
        self.set_workflow(AppWorkflowState::GenerationFailed, now_ms);
        Ok(())
    }

    /// `generation_failed -> generating`. Requires the confirmed revision to
    /// still be the current one — a draft edited after confirmation must be
    /// re-confirmed before regenerating.
    pub fn retry_generation(&mut self, now_ms: u64) -> Result<(), AppError> {
        self.ensure_workflow("retry_generation", &[AppWorkflowState::GenerationFailed])?;
        if self.draft.confirmed_revision != Some(self.draft.revision) {
            return Err(AppError::WorkflowStateInvalid(format!(
                "retry_generation: draft of app {} changed since confirmation (confirmed {:?}, current {}); re-confirm the design first",
                self.record.id, self.draft.confirmed_revision, self.draft.revision
            )));
        }
        self.set_workflow(AppWorkflowState::Generating, now_ms);
        Ok(())
    }

    /// `validating -> awaiting_preview_confirmation`; opens the single
    /// pending preview interaction at the current revision.
    pub fn validation_passed(
        &mut self,
        interaction_id: String,
        now_ms: u64,
    ) -> Result<AppInteractionRequest, AppError> {
        self.ensure_workflow("validation_passed", &[AppWorkflowState::Validating])?;
        let interaction = AppInteractionRequest {
            interaction_id,
            app_id: self.record.id.clone(),
            kind: AppInteractionKind::Preview,
            revision: self.draft.revision,
            created_at_ms: now_ms,
        };
        self.interactions.pending = Some(interaction.clone());
        self.set_workflow(AppWorkflowState::AwaitingPreviewConfirmation, now_ms);
        Ok(interaction)
    }

    /// `validating -> validation_failed`.
    pub fn validation_failed(&mut self, now_ms: u64) -> Result<(), AppError> {
        self.ensure_workflow("validation_failed", &[AppWorkflowState::Validating])?;
        self.set_workflow(AppWorkflowState::ValidationFailed, now_ms);
        Ok(())
    }

    /// `validation_failed -> revising`.
    pub fn begin_revision(&mut self, now_ms: u64) -> Result<(), AppError> {
        self.ensure_workflow("begin_revision", &[AppWorkflowState::ValidationFailed])?;
        self.set_workflow(AppWorkflowState::Revising, now_ms);
        Ok(())
    }

    /// `awaiting_preview_confirmation -> ready`. Requires the exact pending
    /// preview `interaction_id` AND the current draft revision; consumes the
    /// gate and enqueues a `preview_confirmed` continuation.
    pub fn confirm_preview(
        &mut self,
        interaction_id: &str,
        revision: u64,
        now_ms: u64,
    ) -> Result<AppContinuation, AppError> {
        self.ensure_workflow(
            "confirm_preview",
            &[AppWorkflowState::AwaitingPreviewConfirmation],
        )?;
        self.validate_pending(
            "confirm_preview",
            AppInteractionKind::Preview,
            interaction_id,
        )?;
        self.ensure_current_revision(revision)?;
        self.interactions.pending = None;
        self.set_workflow(AppWorkflowState::Ready, now_ms);
        Ok(self.enqueue_continuation(
            AppContinuationKind::PreviewConfirmed,
            serde_json::json!({ "revision": revision }),
            now_ms,
        ))
    }

    /// `awaiting_preview_confirmation | ready -> revising`; voids a pending
    /// preview interaction and enqueues a `revision_requested` continuation
    /// carrying the prompt.
    pub fn request_revision(
        &mut self,
        prompt: &str,
        now_ms: u64,
    ) -> Result<AppContinuation, AppError> {
        self.ensure_workflow(
            "request_revision",
            &[
                AppWorkflowState::AwaitingPreviewConfirmation,
                AppWorkflowState::Ready,
            ],
        )?;
        self.interactions.pending = None;
        self.set_workflow(AppWorkflowState::Revising, now_ms);
        Ok(self.enqueue_continuation(
            AppContinuationKind::RevisionRequested,
            serde_json::json!({ "prompt": prompt }),
            now_ms,
        ))
    }

    /// `revising -> validating`.
    pub fn revision_ready(&mut self, now_ms: u64) -> Result<(), AppError> {
        self.ensure_workflow("revision_ready", &[AppWorkflowState::Revising])?;
        self.set_workflow(AppWorkflowState::Validating, now_ms);
        Ok(())
    }

    /// Enter validation for a checkpoint restore without creating a second
    /// user-revision continuation. The generation coordinator owns the single
    /// durable restore-build job.
    pub fn begin_restore_rebuild(&mut self, now_ms: u64) -> Result<(), AppError> {
        self.ensure_workflow("begin_restore_rebuild", &[AppWorkflowState::Ready])?;
        self.set_workflow(AppWorkflowState::Revising, now_ms);
        Ok(())
    }

    /// Update the runtime record (spec §C). A state change must follow the
    /// runtime transition table; a same-state call just refreshes
    /// `pid`/`last_error`. `port` semantics: `Some(p)` assigns the port when
    /// none is set and must equal the existing one otherwise (a port is NEVER
    /// reassigned — `IndexedDB` origin stability); `None` keeps the current
    /// port. `pid`/`last_error` are overwritten with the given values.
    pub fn set_runtime(
        &mut self,
        next: AppRuntimeState,
        port: Option<u16>,
        pid: Option<u32>,
        last_error: Option<String>,
        now_ms: u64,
    ) -> Result<(), AppError> {
        let current = self.runtime.state;
        if next != current && !runtime_transition_allowed(current, next) {
            return Err(AppError::InvalidRequest(format!(
                "invalid runtime transition {current} -> {next} for app {}",
                self.record.id
            )));
        }
        if let Some(new_port) = port {
            match self.runtime.port {
                Some(existing) if existing != new_port => {
                    return Err(AppError::InvalidRequest(format!(
                        "app {} port is pinned to {existing} and can never be reassigned (requested {new_port})",
                        self.record.id
                    )));
                }
                _ => self.runtime.port = Some(new_port),
            }
        }
        self.runtime.state = next;
        self.runtime.pid = pid;
        self.runtime.last_error = last_error;
        self.runtime.updated_at_ms = now_ms;
        Ok(())
    }

    /// Persist the distribution-selected runtime mode independently from the
    /// process-state transition table.
    pub fn set_runtime_mode(&mut self, mode: AppRuntimeMode, now_ms: u64) {
        self.runtime.mode = Some(mode);
        self.runtime.updated_at_ms = now_ms;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::too_many_lines)]

    use super::*;
    use crate::error::AppErrorCode;
    use crate::questionnaire::{
        AppDesignField, AppDesignFieldOption, AppDesignFieldType, AppDesignStep, AppPlan,
    };
    use crate::types::DesignValue;

    const ALL_WORKFLOW_STATES: [AppWorkflowState; 9] = [
        AppWorkflowState::CollectingSpec,
        AppWorkflowState::AwaitingSpecConfirmation,
        AppWorkflowState::Generating,
        AppWorkflowState::Validating,
        AppWorkflowState::AwaitingPreviewConfirmation,
        AppWorkflowState::Revising,
        AppWorkflowState::Ready,
        AppWorkflowState::GenerationFailed,
        AppWorkflowState::ValidationFailed,
    ];

    /// A brand-new app, fast-forwarded past `authoring_questionnaire` into
    /// `collecting_spec` — the state every pre-existing gating test in this
    /// module was written against, before `authoring_questionnaire` became
    /// the true initial state. Tests that specifically exercise the
    /// authoring/planning states use [`authoring_app`] instead.
    fn app() -> AppState {
        let mut a = AppState::create(
            "abc123".into(),
            "Test".into(),
            "a test app".into(),
            None,
            10,
        );
        a.questionnaire_ready(one_step(), None, 10)
            .expect("fixture questionnaire is valid");
        a
    }

    /// A brand-new app still in `authoring_questionnaire` (the real initial
    /// state) — for tests exercising authoring/planning themselves.
    fn authoring_app() -> AppState {
        AppState::create("notes".into(), "Notes".into(), "一个记事本".into(), None, 1)
    }

    fn one_step() -> Vec<AppDesignStep> {
        vec![AppDesignStep {
            id: "basics".into(),
            order: 0,
            title: "基础".into(),
            description: None,
            fields: vec![AppDesignField {
                id: "tone".into(),
                label: "语气".into(),
                description: None,
                field_type: AppDesignFieldType::SingleChoice,
                required: false,
                allows_custom: false,
                allows_defer: false,
                default_value: None,
                options: vec![AppDesignFieldOption {
                    value: "a".into(),
                    label: "A".into(),
                }],
            }],
        }]
    }

    fn a_plan() -> AppPlan {
        AppPlan {
            collections: Vec::new(),
            capabilities: Vec::new(),
            domains: Vec::new(),
            summary: "s".into(),
        }
    }

    /// App forced into `state`, with a matching pending interaction (and a
    /// confirmed revision) where that state implies one.
    fn app_in(state: AppWorkflowState) -> AppState {
        let mut a = app();
        a.record.workflow_state = state;
        match state {
            AppWorkflowState::AwaitingSpecConfirmation => {
                a.interactions.pending = Some(AppInteractionRequest {
                    interaction_id: "int-designer".into(),
                    app_id: a.record.id.clone(),
                    kind: AppInteractionKind::Designer,
                    revision: a.draft.revision,
                    created_at_ms: 10,
                });
                // Reaching this gate legitimately always goes through
                // `plan_ready`, which stamps `plan_for_revision` — without
                // it `confirm_design`'s freshness gate would refuse every
                // fixture built this way.
                a.draft.plan = Some(a_plan());
                a.draft.plan_for_revision = Some(a.draft.revision);
            }
            AppWorkflowState::AwaitingPreviewConfirmation => {
                a.interactions.pending = Some(AppInteractionRequest {
                    interaction_id: "int-preview".into(),
                    app_id: a.record.id.clone(),
                    kind: AppInteractionKind::Preview,
                    revision: a.draft.revision,
                    created_at_ms: 10,
                });
                a.draft.plan = Some(a_plan());
                a.draft.plan_for_revision = Some(a.draft.revision);
            }
            AppWorkflowState::Generating
            | AppWorkflowState::Validating
            | AppWorkflowState::GenerationFailed
            | AppWorkflowState::ValidationFailed
            | AppWorkflowState::Ready => {
                a.draft.confirmed_revision = Some(a.draft.revision);
                a.draft.plan = Some(a_plan());
                a.draft.plan_for_revision = Some(a.draft.revision);
            }
            AppWorkflowState::CollectingSpec
            | AppWorkflowState::Revising
            | AppWorkflowState::AuthoringQuestionnaire
            | AppWorkflowState::QuestionnaireFailed
            | AppWorkflowState::Planning
            | AppWorkflowState::PlanFailed => {}
        }
        a
    }

    fn set_patch(field: &str, text: &str) -> AppDesignPatch {
        AppDesignPatch {
            ops: vec![AppDesignPatchOp::Set {
                field_id: field.into(),
                value: DesignValue::ShortText(text.into()),
            }],
            note: None,
        }
    }

    /// Assert `op` succeeds exactly in `allowed` states and fails with
    /// `workflow_state_invalid` in every other state.
    fn assert_allowed_exactly(
        allowed: &[AppWorkflowState],
        op: impl Fn(&mut AppState) -> Result<(), AppError>,
        name: &str,
    ) {
        for state in ALL_WORKFLOW_STATES {
            let mut a = app_in(state);
            let result = op(&mut a);
            if allowed.contains(&state) {
                assert!(result.is_ok(), "{name} should be allowed in {state}");
            } else {
                let err = result.expect_err(&format!("{name} should fail in {state}"));
                assert_eq!(
                    err.code(),
                    AppErrorCode::WorkflowStateInvalid,
                    "{name} in {state} returned wrong code: {err}"
                );
            }
        }
    }

    #[test]
    fn open_designer_reopens_a_failed_generation() {
        assert_allowed_exactly(
            &[
                AppWorkflowState::CollectingSpec,
                AppWorkflowState::GenerationFailed,
            ],
            |a| a.open_designer("int-1".into(), 11).map(|_| ()),
            "open_designer",
        );
        let mut a = app();
        let interaction = a.open_designer("int-1".into(), 11).unwrap();
        assert_eq!(interaction.kind, AppInteractionKind::Designer);
        assert_eq!(interaction.revision, 0);
        assert_eq!(
            a.record.workflow_state,
            AppWorkflowState::AwaitingSpecConfirmation
        );
        assert_eq!(
            a.interactions.pending.as_ref().unwrap().interaction_id,
            "int-1"
        );
    }

    #[test]
    fn update_draft_allowed_states_and_effects() {
        assert_allowed_exactly(
            &DRAFT_EDITABLE_STATES,
            |a| a.update_draft(0, &set_patch("t", "x"), 11).map(|_| ()),
            "update_draft",
        );
        let mut a = app();
        assert_eq!(
            a.update_draft(0, &set_patch("title", "Board"), 11).unwrap(),
            1
        );
        assert_eq!(
            a.draft.fields.get("title"),
            Some(&DesignValue::ShortText("Board".into()))
        );
        // Remove is a no-op for absent fields, set overwrites.
        let patch = AppDesignPatch {
            ops: vec![
                AppDesignPatchOp::Remove {
                    field_id: "ghost".into(),
                },
                AppDesignPatchOp::Set {
                    field_id: "title".into(),
                    value: DesignValue::ShortText("Board 2".into()),
                },
            ],
            note: Some("edit".into()),
        };
        assert_eq!(a.update_draft(1, &patch, 12).unwrap(), 2);
        assert_eq!(
            a.draft.fields.get("title"),
            Some(&DesignValue::ShortText("Board 2".into()))
        );
        // State unchanged by draft edits.
        assert_eq!(a.record.workflow_state, AppWorkflowState::CollectingSpec);
    }

    #[test]
    fn update_draft_revision_conflict_keeps_user_value() {
        let mut a = app();
        a.update_draft(0, &set_patch("title", "Mine"), 11).unwrap();
        let err = a
            .update_draft(0, &set_patch("title", "Stale"), 12)
            .unwrap_err();
        assert!(matches!(
            err,
            AppError::RevisionConflict {
                expected: 0,
                actual: 1
            }
        ));
        // No silent overwrite: value and revision are untouched.
        assert_eq!(
            a.draft.fields.get("title"),
            Some(&DesignValue::ShortText("Mine".into()))
        );
        assert_eq!(a.draft.revision, 1);
    }

    #[test]
    fn pending_designer_interaction_survives_updates_but_confirm_needs_current_revision() {
        let mut a = app();
        let interaction = a.open_designer("int-1".into(), 11).unwrap();
        a.update_draft(0, &set_patch("title", "v1"), 12).unwrap();
        assert!(
            a.interactions.pending.is_some(),
            "interaction must survive edits"
        );
        // Confirming with the interaction's original (now stale) revision fails.
        let err = a
            .confirm_design(&interaction.interaction_id, interaction.revision, 13)
            .unwrap_err();
        assert_eq!(err.code(), AppErrorCode::RevisionConflict);
        assert!(
            a.interactions.pending.is_some(),
            "failed confirm must not consume the gate"
        );
        // This test is about revision matching, not plan freshness — stamp a
        // plan for the edited revision directly (bypassing `plan_ready`,
        // which isn't what's under test here).
        a.draft.plan_for_revision = Some(a.draft.revision);
        // Confirming with the CURRENT revision succeeds.
        a.confirm_design(&interaction.interaction_id, 1, 14)
            .unwrap();
        assert_eq!(a.record.workflow_state, AppWorkflowState::Generating);
        assert_eq!(a.draft.confirmed_revision, Some(1));
        assert!(a.interactions.pending.is_none());
    }

    #[test]
    fn store_and_apply_suggestion_lifecycle() {
        assert_allowed_exactly(
            &DRAFT_EDITABLE_STATES,
            |a| {
                a.store_suggestion("sugg-1".into(), set_patch("t", "x"), 11)
                    .map(|_| ())
            },
            "store_suggestion",
        );
        let mut a = app();
        let suggestion = a
            .store_suggestion("sugg-1".into(), set_patch("accent", "#3366ff"), 11)
            .unwrap();
        assert_eq!(suggestion.based_on_revision, 0);
        // Storing does not touch fields or revision.
        assert_eq!(a.draft.revision, 0);
        assert!(a.draft.fields.is_empty());
        // Replacing the stored suggestion is allowed.
        a.store_suggestion("sugg-2".into(), set_patch("accent", "#112233"), 12)
            .unwrap();
        // Wrong suggestion id: interaction_invalid, suggestion survives.
        let err = a.apply_suggestion("sugg-1", 0, 13).unwrap_err();
        assert_eq!(err.code(), AppErrorCode::InteractionInvalid);
        assert!(a.draft.pending_suggestion.is_some());
        // Stale revision: revision_conflict, suggestion survives.
        let err = a.apply_suggestion("sugg-2", 9, 14).unwrap_err();
        assert_eq!(err.code(), AppErrorCode::RevisionConflict);
        assert!(a.draft.pending_suggestion.is_some());
        // Correct id + revision: applied and consumed.
        assert_eq!(a.apply_suggestion("sugg-2", 0, 15).unwrap(), 1);
        assert_eq!(
            a.draft.fields.get("accent"),
            Some(&DesignValue::ShortText("#112233".into()))
        );
        assert!(a.draft.pending_suggestion.is_none());
        // Applying again: nothing pending.
        let err = a.apply_suggestion("sugg-2", 1, 16).unwrap_err();
        assert_eq!(err.code(), AppErrorCode::InteractionInvalid);
    }

    #[test]
    fn dismiss_suggestion_consumes_only_the_matching_id_without_revision_change() {
        let mut app = app_in(AppWorkflowState::CollectingSpec);
        app.store_suggestion("sugg-1".into(), set_patch("accent", "#112233"), 11)
            .unwrap();
        let error = app.dismiss_suggestion("wrong", 12).unwrap_err();
        assert_eq!(error.code(), crate::error::AppErrorCode::InteractionInvalid);
        assert!(app.draft.pending_suggestion.is_some());
        app.dismiss_suggestion("sugg-1", 13).unwrap();
        assert!(app.draft.pending_suggestion.is_none());
        assert_eq!(app.draft.revision, 0);
        assert_eq!(app.record.updated_at_ms, 13);
    }

    #[test]
    fn apply_suggestion_state_gating() {
        assert_allowed_exactly(
            &DRAFT_EDITABLE_STATES,
            |a| {
                a.draft.pending_suggestion = Some(AppDesignSuggestion {
                    suggestion_id: "sugg-1".into(),
                    patch: set_patch("t", "x"),
                    based_on_revision: 0,
                });
                a.apply_suggestion("sugg-1", 0, 11).map(|_| ())
            },
            "apply_suggestion",
        );
    }

    #[test]
    fn confirm_design_gating() {
        assert_allowed_exactly(
            &[AppWorkflowState::AwaitingSpecConfirmation],
            |a| a.confirm_design("int-designer", 0, 11).map(|_| ()),
            "confirm_design",
        );
        // Wrong interaction id fails and leaves the gate pending.
        let mut a = app_in(AppWorkflowState::AwaitingSpecConfirmation);
        let err = a.confirm_design("int-guessed", 0, 11).unwrap_err();
        assert_eq!(err.code(), AppErrorCode::InteractionInvalid);
        assert!(a.interactions.pending.is_some());
        // No pending interaction at all (state forced by hand).
        let mut a = app_in(AppWorkflowState::AwaitingSpecConfirmation);
        a.interactions.pending = None;
        let err = a.confirm_design("int-designer", 0, 11).unwrap_err();
        assert_eq!(err.code(), AppErrorCode::InteractionInvalid);
        // A pending PREVIEW interaction does not satisfy confirm_design.
        let mut a = app_in(AppWorkflowState::AwaitingSpecConfirmation);
        a.interactions.pending = Some(AppInteractionRequest {
            interaction_id: "int-preview".into(),
            app_id: a.record.id.clone(),
            kind: AppInteractionKind::Preview,
            revision: 0,
            created_at_ms: 10,
        });
        let err = a.confirm_design("int-preview", 0, 11).unwrap_err();
        assert_eq!(err.code(), AppErrorCode::InteractionInvalid);
    }

    #[test]
    fn confirm_design_enqueues_continuation() {
        let mut a = app_in(AppWorkflowState::AwaitingSpecConfirmation);
        let continuation = a.confirm_design("int-designer", 0, 11).unwrap();
        assert_eq!(continuation.seq, 1);
        assert_eq!(continuation.kind, AppContinuationKind::DesignConfirmed);
        assert_eq!(continuation.payload, serde_json::json!({ "revision": 0 }));
        assert_eq!(a.interactions.next_seq, 2);
        assert_eq!(a.interactions.undelivered.len(), 1);
    }

    #[test]
    fn cancel_design_voids_gate_and_returns_to_collecting() {
        assert_allowed_exactly(
            &[AppWorkflowState::AwaitingSpecConfirmation],
            |a| a.cancel_design(11).map(|_| ()),
            "cancel_design",
        );
        let mut a = app_in(AppWorkflowState::AwaitingSpecConfirmation);
        let continuation = a.cancel_design(11).unwrap();
        assert_eq!(continuation.kind, AppContinuationKind::DesignCancelled);
        assert_eq!(a.record.workflow_state, AppWorkflowState::CollectingSpec);
        assert!(a.interactions.pending.is_none());
    }

    #[test]
    fn generation_and_validation_transitions() {
        assert_allowed_exactly(
            &[AppWorkflowState::Generating],
            |a| a.generation_complete(11),
            "generation_complete",
        );
        assert_allowed_exactly(
            &[AppWorkflowState::Generating],
            |a| a.generation_failed(11),
            "generation_failed",
        );
        assert_allowed_exactly(
            &[AppWorkflowState::GenerationFailed],
            |a| a.retry_generation(11),
            "retry_generation",
        );
        assert_allowed_exactly(
            &[AppWorkflowState::Validating],
            |a| a.validation_passed("int-p".into(), 11).map(|_| ()),
            "validation_passed",
        );
        assert_allowed_exactly(
            &[AppWorkflowState::Validating],
            |a| a.validation_failed(11),
            "validation_failed",
        );
        assert_allowed_exactly(
            &[AppWorkflowState::ValidationFailed],
            |a| a.begin_revision(11),
            "begin_revision",
        );
        assert_allowed_exactly(
            &[AppWorkflowState::Revising],
            |a| a.revision_ready(11),
            "revision_ready",
        );
    }

    #[test]
    fn retry_generation_requires_reconfirmed_draft() {
        let mut a = app_in(AppWorkflowState::GenerationFailed);
        // Simulate a draft that drifted past its confirmation.
        a.draft.revision += 1;
        let err = a.retry_generation(11).unwrap_err();
        assert_eq!(err.code(), AppErrorCode::WorkflowStateInvalid);
        assert_eq!(a.record.workflow_state, AppWorkflowState::GenerationFailed);
        // With confirmed == current it proceeds.
        a.draft.confirmed_revision = Some(a.draft.revision);
        a.retry_generation(12).unwrap();
        assert_eq!(a.record.workflow_state, AppWorkflowState::Generating);
    }

    /// A generation failure caused by the DESIGN must be fixable: re-opening
    /// the designer returns the draft to an editable state, and the fixed
    /// draft confirms into a fresh generation.
    #[test]
    fn generation_failed_recovers_through_the_designer() {
        let mut a = app_in(AppWorkflowState::GenerationFailed);
        let gate = a.open_designer("int-fix".into(), 11).unwrap();
        assert_eq!(
            a.record.workflow_state,
            AppWorkflowState::AwaitingSpecConfirmation
        );
        // Editing invalidates the stamped plan (`update_draft` clears
        // `plan_for_revision`), and there is no path from
        // `awaiting_spec_confirmation` back to `planning` — so recovering
        // now goes: cancel back to `collecting_spec`, fix the answer,
        // re-plan, then confirm the FRESH plan. `confirm_design` refuses a
        // stale one on purpose.
        a.cancel_design(12).unwrap();
        assert!(a.interactions.pending.is_none(), "the old gate is voided");
        let _ = gate;
        let current = a.draft.revision;
        let revision = a
            .update_draft(current, &set_patch("title", "fixed"), 13)
            .unwrap();
        a.begin_planning(14).unwrap();
        let fresh_gate = a
            .plan_ready(a_plan(), "int-fix-2".into(), 15)
            .expect("re-planning the fixed draft succeeds");
        assert_eq!(a.draft.plan_for_revision, Some(revision));
        a.confirm_design(&fresh_gate.interaction_id, revision, 16)
            .unwrap();
        assert_eq!(a.record.workflow_state, AppWorkflowState::Generating);
        assert_eq!(a.draft.confirmed_revision, Some(revision));
    }

    #[test]
    fn validation_passed_opens_preview_gate() {
        let mut a = app_in(AppWorkflowState::Validating);
        let interaction = a.validation_passed("int-p".into(), 11).unwrap();
        assert_eq!(interaction.kind, AppInteractionKind::Preview);
        assert_eq!(
            a.record.workflow_state,
            AppWorkflowState::AwaitingPreviewConfirmation
        );
        assert_eq!(
            a.interactions.pending.as_ref().unwrap().interaction_id,
            "int-p"
        );
    }

    #[test]
    fn confirm_preview_gating_and_continuation() {
        assert_allowed_exactly(
            &[AppWorkflowState::AwaitingPreviewConfirmation],
            |a| a.confirm_preview("int-preview", 0, 11).map(|_| ()),
            "confirm_preview",
        );
        let mut a = app_in(AppWorkflowState::AwaitingPreviewConfirmation);
        let err = a.confirm_preview("int-guessed", 0, 11).unwrap_err();
        assert_eq!(err.code(), AppErrorCode::InteractionInvalid);
        let err = a.confirm_preview("int-preview", 3, 11).unwrap_err();
        assert_eq!(err.code(), AppErrorCode::RevisionConflict);
        assert!(a.interactions.pending.is_some());
        let continuation = a.confirm_preview("int-preview", 0, 12).unwrap();
        assert_eq!(continuation.kind, AppContinuationKind::PreviewConfirmed);
        assert_eq!(a.record.workflow_state, AppWorkflowState::Ready);
        assert!(a.interactions.pending.is_none());
    }

    #[test]
    fn request_revision_from_preview_gate_and_ready() {
        assert_allowed_exactly(
            &[
                AppWorkflowState::AwaitingPreviewConfirmation,
                AppWorkflowState::Ready,
            ],
            |a| a.request_revision("darker header", 11).map(|_| ()),
            "request_revision",
        );
        // From the preview gate it voids the pending interaction.
        let mut a = app_in(AppWorkflowState::AwaitingPreviewConfirmation);
        let continuation = a.request_revision("darker header", 11).unwrap();
        assert_eq!(continuation.kind, AppContinuationKind::RevisionRequested);
        assert_eq!(
            continuation.payload,
            serde_json::json!({ "prompt": "darker header" })
        );
        assert!(a.interactions.pending.is_none());
        assert_eq!(a.record.workflow_state, AppWorkflowState::Revising);
        // The voided preview gate cannot be confirmed afterwards.
        let err = a.confirm_preview("int-preview", 0, 12).unwrap_err();
        assert_eq!(err.code(), AppErrorCode::WorkflowStateInvalid);
    }

    #[test]
    fn continuation_seqs_are_monotonic_across_gates() {
        let mut a = app();
        a.open_designer("int-1".into(), 11).unwrap();
        let c1 = a.cancel_design(12).unwrap();
        a.open_designer("int-2".into(), 13).unwrap();
        // Not testing planning here — stamp a plan for the current revision
        // so the freshness gate `confirm_design` added doesn't get in the
        // way of the seq-monotonicity assertion this test is actually for.
        a.draft.plan_for_revision = Some(a.draft.revision);
        let c2 = a.confirm_design("int-2", 0, 14).unwrap();
        a.generation_complete(15).unwrap();
        let p = a.validation_passed("int-3".into(), 16).unwrap();
        let c3 = a.confirm_preview(&p.interaction_id, 0, 17).unwrap();
        let c4 = a.request_revision("more contrast", 18).unwrap();
        assert_eq!(
            (c1.seq, c2.seq, c3.seq, c4.seq),
            (1, 2, 3, 4),
            "seqs must increase monotonically"
        );
        assert_eq!(a.interactions.next_seq, 5);
        assert_eq!(a.interactions.undelivered.len(), 4);
    }

    #[test]
    fn runtime_transition_table_is_exhaustive() {
        use AppRuntimeState::{Failed, Running, Starting, Stopped, Stopping};
        let all = [Stopped, Starting, Running, Stopping, Failed];
        let legal = [
            (Stopped, Starting),
            (Starting, Running),
            (Running, Stopping),
            (Stopping, Stopped),
            (Starting, Failed),
            (Running, Failed),
            (Failed, Starting),
        ];
        for from in all {
            for to in all {
                let expected = legal.contains(&(from, to));
                assert_eq!(
                    runtime_transition_allowed(from, to),
                    expected,
                    "transition {from} -> {to}"
                );
                if from == to {
                    continue; // same-state is a record refresh, not a transition
                }
                let mut a = app();
                a.runtime.state = from;
                let result = a.set_runtime(to, None, None, None, 11);
                if expected {
                    assert!(result.is_ok(), "{from} -> {to} should be legal");
                    assert_eq!(a.runtime.state, to);
                } else {
                    let err = result.expect_err(&format!("{from} -> {to} should be illegal"));
                    assert_eq!(err.code(), AppErrorCode::InvalidRequest);
                    assert_eq!(a.runtime.state, from, "failed transition must not commit");
                }
            }
        }
    }

    #[test]
    fn runtime_same_state_refreshes_record() {
        let mut a = app();
        a.set_runtime(AppRuntimeState::Starting, Some(3005), Some(77), None, 11)
            .unwrap();
        a.set_runtime(
            AppRuntimeState::Starting,
            None,
            Some(78),
            Some("slow boot".into()),
            12,
        )
        .unwrap();
        assert_eq!(a.runtime.pid, Some(78));
        assert_eq!(a.runtime.last_error.as_deref(), Some("slow boot"));
        assert_eq!(a.runtime.port, Some(3005), "None keeps the pinned port");
    }

    /// Finding 2: with a persistently dead sink the undelivered queue caps at
    /// [`MAX_UNDELIVERED_CONTINUATIONS`], dropping the OLDEST entry so the
    /// newest gate outcome always survives.
    #[test]
    fn undelivered_queue_caps_at_max_dropping_oldest() {
        let mut a = app();
        let cycles = u64::try_from(MAX_UNDELIVERED_CONTINUATIONS).unwrap() + 1;
        for i in 0..cycles {
            a.open_designer(format!("int-{i}"), 100 + i).unwrap();
            a.cancel_design(101 + i).unwrap();
        }
        assert_eq!(
            a.interactions.undelivered.len(),
            MAX_UNDELIVERED_CONTINUATIONS,
            "queue must stay capped"
        );
        let seqs: Vec<u64> = a.interactions.undelivered.iter().map(|c| c.seq).collect();
        assert_eq!(
            seqs.first(),
            Some(&2),
            "the OLDEST entry (seq 1) is dropped"
        );
        assert_eq!(seqs.last(), Some(&cycles), "the newest entry survives");
        assert!(
            seqs.windows(2).all(|pair| pair[0] < pair[1]),
            "queue order stays strictly increasing"
        );
        assert_eq!(
            a.interactions.next_seq,
            cycles + 1,
            "dropping never rewinds the seq counter"
        );
    }

    /// Finding 4: the byte budget is the PRIMARY cap — big legal payloads
    /// trip it long before the count cap, dropping oldest until the summed
    /// serialized size fits, so `interactions.json` can never grow toward
    /// the document bound.
    #[test]
    fn undelivered_queue_byte_budget_drops_oldest_before_the_count_cap() {
        let mut a = app_in(AppWorkflowState::Ready);
        // ~120 KB serialized per continuation (20 000 control chars escape
        // 6×) — the 4 MiB budget admits ~35 of them, far below 256.
        let prompt = "\u{1}".repeat(20_000);
        for cycle in 0..40u64 {
            a.request_revision(&prompt, 100 + cycle).unwrap();
            a.revision_ready(101 + cycle).unwrap();
            a.validation_passed(format!("int-{cycle}"), 102 + cycle)
                .unwrap();
            a.confirm_preview(&format!("int-{cycle}"), 0, 103 + cycle)
                .unwrap();
        }
        let minted = usize::try_from(a.interactions.next_seq - 1).unwrap();
        assert_eq!(minted, 80, "40 heavy + 40 small continuations were minted");
        let queue = &a.interactions.undelivered;
        assert!(
            queue.len() < minted,
            "the byte budget must have dropped oldest entries"
        );
        assert!(
            queue.first().map(|c| c.seq) > Some(1),
            "oldest seqs dropped first"
        );
        let total: usize = queue
            .iter()
            .map(|c| serde_json::to_string(c).unwrap().len())
            .sum();
        assert!(
            total <= MAX_UNDELIVERED_BYTES,
            "{total} serialized bytes exceed the budget"
        );
        assert!(
            queue.len() <= MAX_UNDELIVERED_CONTINUATIONS,
            "the count cap stays a secondary bound"
        );
        assert!(
            queue.windows(2).all(|pair| pair[0].seq < pair[1].seq),
            "queue order stays strictly increasing"
        );
    }

    /// Finding 8 (aggregate level): a suggestion whose `based_on_revision`
    /// trails the current revision is refused with the typed conflict AND
    /// consumed; fields and revision stay untouched.
    #[test]
    fn stale_based_on_revision_is_refused_and_consumed() {
        let mut a = app();
        a.store_suggestion("sugg-1".into(), set_patch("accent", "#old"), 11)
            .unwrap();
        // The user edits past the suggestion's basis.
        a.update_draft(0, &set_patch("title", "Mine"), 12).unwrap();
        a.update_draft(1, &set_patch("title", "Mine v2"), 13)
            .unwrap();
        // Caller passes the CURRENT revision — only the suggestion is stale.
        let err = a.apply_suggestion("sugg-1", 2, 14).unwrap_err();
        assert!(
            matches!(
                err,
                AppError::RevisionConflict {
                    expected: 0,
                    actual: 2
                }
            ),
            "conflict must carry based_on vs current: {err}"
        );
        assert!(
            a.draft.pending_suggestion.is_none(),
            "stale suggestion consumed"
        );
        assert_eq!(a.draft.revision, 2, "revision untouched");
        assert_eq!(
            a.draft.fields.get("title"),
            Some(&DesignValue::ShortText("Mine v2".into())),
            "user fields untouched"
        );
        assert!(!a.draft.fields.contains_key("accent"), "nothing applied");
    }

    #[test]
    fn runtime_port_is_never_reassigned() {
        let mut a = app();
        a.set_runtime(AppRuntimeState::Starting, Some(3005), None, None, 11)
            .unwrap();
        // Same port is fine.
        a.set_runtime(AppRuntimeState::Running, Some(3005), Some(1), None, 12)
            .unwrap();
        // A different port is rejected and nothing else changes.
        let err = a
            .set_runtime(AppRuntimeState::Failed, Some(3006), None, None, 13)
            .unwrap_err();
        assert_eq!(err.code(), AppErrorCode::InvalidRequest);
        assert_eq!(a.runtime.port, Some(3005));
        assert_eq!(a.runtime.state, AppRuntimeState::Running);
    }

    #[test]
    fn a_new_app_starts_in_authoring_questionnaire() {
        assert_eq!(
            authoring_app().record.workflow_state,
            AppWorkflowState::AuthoringQuestionnaire
        );
    }

    #[test]
    fn questionnaire_ready_moves_to_collecting_spec_and_stores_the_steps() {
        let mut app = authoring_app();
        app.questionnaire_ready(one_step(), Some("记事本".into()), 2)
            .expect("authoring succeeds");
        assert_eq!(app.record.workflow_state, AppWorkflowState::CollectingSpec);
        assert_eq!(app.draft.questionnaire.len(), 1);
        assert_eq!(app.record.name, "记事本", "a suggested name replaces the placeholder");
    }

    #[test]
    fn a_failed_authoring_retry_returns_to_authoring_not_to_collecting_spec() {
        let mut app = authoring_app();
        app.questionnaire_failed(2).expect("authoring can fail");
        assert_eq!(app.record.workflow_state, AppWorkflowState::QuestionnaireFailed);
        app.retry_questionnaire(3)
            .expect("a failed authoring can be retried");
        assert_eq!(
            app.record.workflow_state,
            AppWorkflowState::AuthoringQuestionnaire,
            "retry re-runs authoring; it does not skip ahead"
        );
    }

    #[test]
    fn updating_the_brief_clears_the_questionnaire_answers_and_plan() {
        let mut app = authoring_app();
        app.questionnaire_ready(one_step(), None, 2)
            .expect("authoring succeeds");
        app.draft
            .fields
            .insert("tone".into(), DesignValue::SingleChoice("a".into()));
        app.draft.plan = Some(a_plan());
        app.draft.plan_for_revision = Some(app.draft.revision);

        app.update_brief("换成一个待办清单".into(), 3)
            .expect("brief is editable while collecting");

        assert_eq!(app.record.brief, "换成一个待办清单");
        assert!(
            app.draft.questionnaire.is_empty(),
            "the questionnaire must be re-authored"
        );
        assert!(
            app.draft.fields.is_empty(),
            "old answers reference field ids that no longer exist"
        );
        assert!(app.draft.plan.is_none());
        assert_eq!(
            app.record.workflow_state,
            AppWorkflowState::AuthoringQuestionnaire
        );
    }

    #[test]
    fn the_brief_is_not_editable_once_generation_has_been_confirmed() {
        let mut app = authoring_app();
        app.questionnaire_ready(one_step(), None, 2)
            .expect("authoring succeeds");
        app.begin_planning(3).expect("planning starts");
        app.plan_ready(a_plan(), "i-1".into(), 4)
            .expect("planning succeeds");
        app.confirm_design("i-1", app.draft.revision, 5)
            .expect("the user confirms");
        app.update_brief("太晚了".into(), 6)
            .expect_err("the brief is frozen after confirmation");
    }

    #[test]
    fn plan_ready_opens_the_spec_confirmation_gate_and_stamps_the_revision() {
        let mut app = authoring_app();
        app.questionnaire_ready(one_step(), None, 2)
            .expect("authoring succeeds");
        app.begin_planning(3).expect("planning starts");
        let interaction = app
            .plan_ready(a_plan(), "i-1".into(), 4)
            .expect("planning succeeds");

        assert_eq!(
            app.record.workflow_state,
            AppWorkflowState::AwaitingSpecConfirmation
        );
        assert_eq!(interaction.kind, AppInteractionKind::Designer);
        assert_eq!(app.draft.plan_for_revision, Some(app.draft.revision));
    }

    #[test]
    fn confirming_a_plan_computed_for_an_older_revision_is_refused() {
        let mut app = authoring_app();
        app.questionnaire_ready(one_step(), None, 2)
            .expect("authoring succeeds");
        app.begin_planning(3).expect("planning starts");
        app.plan_ready(a_plan(), "i-1".into(), 4)
            .expect("planning succeeds");

        // 用户回头改了一个答案：revision 前进，方案作废。
        app.draft.revision += 1;

        let error = app
            .confirm_design("i-1", app.draft.revision, 5)
            .expect_err("a stale plan must never be confirmed");
        assert!(
            matches!(error, AppError::RevisionConflict { .. }),
            "got {error}"
        );
    }

    #[test]
    fn a_failed_plan_retry_returns_to_planning() {
        let mut app = authoring_app();
        app.questionnaire_ready(one_step(), None, 2)
            .expect("authoring succeeds");
        app.begin_planning(3).expect("planning starts");
        app.plan_failed(4).expect("planning can fail");
        assert_eq!(app.record.workflow_state, AppWorkflowState::PlanFailed);
        app.retry_plan(5).expect("a failed plan can be retried");
        assert_eq!(app.record.workflow_state, AppWorkflowState::Planning);
    }

    #[test]
    fn the_draft_is_read_only_while_the_llm_is_authoring_or_planning() {
        let patch = AppDesignPatch {
            ops: vec![AppDesignPatchOp::Set {
                field_id: "tone".into(),
                value: DesignValue::SingleChoice("a".into()),
            }],
            note: None,
        };

        let mut app = authoring_app();
        app.update_draft(0, &patch, 2)
            .expect_err("no edits while authoring — the answers would race the questions");

        let mut app = authoring_app();
        app.questionnaire_ready(one_step(), None, 2)
            .expect("authoring succeeds");
        app.begin_planning(3).expect("planning starts");
        app.update_draft(app.draft.revision, &patch, 4)
            .expect_err("no edits while planning — the plan would be computed against stale answers");
    }
}
