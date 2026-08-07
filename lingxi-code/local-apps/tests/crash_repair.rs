//! Torn-commit repair goldens (storage module doc, "For MUTATIONS…").
//!
//! `with_app` persists the per-app batch (`design-spec.json`,
//! `interactions.json`, `runtime.json`, `app.json` mirror — in that order)
//! BEFORE `index.json`. A process kill inside that window used to reload into
//! a torn store: the index still showed the pre-transition state while the
//! gate was already consumed and its continuation queued — an unsatisfiable
//! confirmation gate plus a phantom continuation. These tests pin
//! `storage::load_all`'s forward repair at every crash point, and that the
//! repair is PERSISTED (a second load sees a consistent store).

use local_apps::storage::{
    self, load_all, save_app_files, save_app_files_steps, save_index, save_interactions,
    write_order_prefix_through, AppDocWriteStep,
};
use local_apps::test_support::FixedClock;
use local_apps::{
    AppContinuationKind, AppDesignField, AppDesignFieldOption, AppDesignFieldType, AppDesignStep,
    AppEventObserver, AppInteractionKind, AppInteractions, AppPlan, AppService, AppState,
    AppWorkflowState, ContinuationSink, RecordingAppEventObserver, RecordingContinuationSink,
    APPS_SCHEMA_VERSION,
};
use std::path::Path;
use std::sync::Arc;

const T0: u64 = 1_753_000_000_000;

fn one_step() -> Vec<AppDesignStep> {
    vec![AppDesignStep {
        id: "basics".into(),
        order: 0,
        title: "basics".into(),
        description: None,
        fields: vec![AppDesignField {
            id: "tone".into(),
            label: "tone".into(),
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

/// A fresh app fast-forwarded straight to `collecting_spec` WITH an
/// already-computed plan matching the current revision — none of these
/// crash-repair goldens exercise questionnaire authoring or planning
/// themselves, they pin the torn-write repair machinery, so the fixture
/// just needs to satisfy `confirm_design`'s plan-freshness gate without
/// ever touching `update_draft` (which would invalidate it again).
fn fresh_app(id: &str) -> AppState {
    let mut app = AppState::create(
        id.into(),
        format!("App {id}"),
        "a test app".into(),
        None,
        T0,
    );
    app.questionnaire_ready(one_step(), None, T0)
        .expect("fixture questionnaire is valid");
    app.draft.plan = Some(a_plan());
    app.draft.plan_for_revision = Some(app.draft.revision);
    app
}

fn a_plan() -> AppPlan {
    AppPlan {
        collections: Vec::new(),
        capabilities: Vec::new(),
        domains: Vec::new(),
        summary: "s".into(),
    }
}

/// Persist the full consistent store — the committed state BEFORE the torn
/// mutation began (per-app batch + index, the service's write order).
fn commit_full(root: &Path, app: &AppState) {
    save_app_files(root, app).expect("save app files");
    save_index(root, &[app.record.clone()]).expect("save index");
}

/// Where the crash landed inside one torn mutation.
enum Tear {
    /// The whole per-app batch (incl. the `app.json` mirror) committed; only
    /// the index write was lost.
    AfterAppFiles,
    /// Only the batch prefix up to `interactions.json` committed; the mirror
    /// and the index still show the pre-transition record.
    AfterInteractions,
}

/// Commit `before` fully, then replay `after`'s writes up to the crash point
/// as a TRUE prefix of the real write sequence
/// ([`local_apps::storage::APP_DOC_WRITE_ORDER`]) — never a hand-assumed
/// order.
fn tear(root: &Path, before: &AppState, after: &AppState, at: &Tear) {
    commit_full(root, before);
    match at {
        Tear::AfterAppFiles => save_app_files(root, after).expect("save torn app files"),
        Tear::AfterInteractions => save_app_files_steps(
            root,
            after,
            write_order_prefix_through(AppDocWriteStep::Interactions),
        )
        .expect("save torn prefix"),
    }
}

/// Load twice: the first load repairs AND persists, the second must see an
/// already-consistent store with the same result.
fn load_repaired(root: &Path) -> AppState {
    let first = load_all(root).expect("torn store must load");
    assert_eq!(first.len(), 1);
    let second = load_all(root).expect("repaired store must load");
    assert_eq!(first, second, "repair must be persisted, not re-derived");
    first.into_iter().next().expect("one app")
}

#[test]
fn torn_confirm_design_after_full_batch_repairs_to_generating() {
    let dir = tempfile::tempdir().unwrap();
    let mut before = fresh_app("aaaa1111");
    before.open_designer("int-1".into(), T0 + 1).unwrap();
    let mut after = before.clone();
    after.confirm_design("int-1", 0, T0 + 2).unwrap();
    tear(dir.path(), &before, &after, &Tear::AfterAppFiles);

    let app = load_repaired(dir.path());
    assert_eq!(app.record.workflow_state, AppWorkflowState::Generating);
    assert!(app.interactions.pending.is_none());
    assert_eq!(app.draft.confirmed_revision, Some(0));
    // The committed continuation is now truthful, not phantom.
    assert_eq!(app.interactions.undelivered.len(), 1);
    assert_eq!(
        app.interactions.undelivered[0].kind,
        AppContinuationKind::DesignConfirmed
    );
    // The repaired index landed on disk.
    let index = std::fs::read_to_string(dir.path().join("apps/index.json")).unwrap();
    assert!(index.contains("\"generating\""), "{index}");
}

#[test]
fn torn_confirm_design_mid_batch_repairs_to_generating() {
    let dir = tempfile::tempdir().unwrap();
    let mut before = fresh_app("aaaa1111");
    before.open_designer("int-1".into(), T0 + 1).unwrap();
    let mut after = before.clone();
    after.confirm_design("int-1", 0, T0 + 2).unwrap();
    tear(dir.path(), &before, &after, &Tear::AfterInteractions);

    let app = load_repaired(dir.path());
    assert_eq!(app.record.workflow_state, AppWorkflowState::Generating);
    assert!(app.interactions.pending.is_none());
    assert_eq!(app.draft.confirmed_revision, Some(0));
}

#[test]
fn torn_cancel_design_repairs_to_collecting_spec() {
    let dir = tempfile::tempdir().unwrap();
    let mut before = fresh_app("aaaa1111");
    before.open_designer("int-1".into(), T0 + 1).unwrap();
    let mut after = before.clone();
    after.cancel_design(T0 + 2).unwrap();
    tear(dir.path(), &before, &after, &Tear::AfterInteractions);

    let app = load_repaired(dir.path());
    assert_eq!(app.record.workflow_state, AppWorkflowState::CollectingSpec);
    assert!(app.interactions.pending.is_none());
    assert_eq!(
        app.interactions.undelivered.last().map(|c| c.kind),
        Some(AppContinuationKind::DesignCancelled)
    );
}

#[test]
fn torn_open_designer_repairs_to_awaiting_spec_confirmation() {
    let dir = tempfile::tempdir().unwrap();
    let before = fresh_app("aaaa1111");
    let mut after = before.clone();
    after.open_designer("int-1".into(), T0 + 1).unwrap();
    tear(dir.path(), &before, &after, &Tear::AfterInteractions);

    let mut app = load_repaired(dir.path());
    assert_eq!(
        app.record.workflow_state,
        AppWorkflowState::AwaitingSpecConfirmation
    );
    let pending = app.interactions.pending.clone().expect("gate survives");
    assert_eq!(pending.interaction_id, "int-1");
    // The repaired gate is actually satisfiable.
    app.confirm_design("int-1", 0, T0 + 3).expect("confirmable");
}

/// `plan_ready` (Task 3) arms the SAME Designer gate `open_designer` does,
/// but from `Planning` — a third source state the repair table's
/// `(CollectingSpec | GenerationFailed, Some(Designer))` pattern originally
/// missed. A crash in this window would otherwise wedge `confirm_design`
/// behind a `Planning` record the table never resolves.
#[test]
fn torn_plan_ready_repairs_to_awaiting_spec_confirmation() {
    let dir = tempfile::tempdir().unwrap();
    let mut before = fresh_app("aaaa1111");
    before.begin_planning(T0 + 1).unwrap();
    let mut after = before.clone();
    after.plan_ready(a_plan(), "int-1".into(), T0 + 2).unwrap();
    tear(dir.path(), &before, &after, &Tear::AfterInteractions);

    let mut app = load_repaired(dir.path());
    assert_eq!(
        app.record.workflow_state,
        AppWorkflowState::AwaitingSpecConfirmation
    );
    let pending = app.interactions.pending.clone().expect("gate survives");
    assert_eq!(pending.interaction_id, "int-1");
    // The repaired gate is actually satisfiable.
    app.confirm_design("int-1", app.draft.revision, T0 + 3)
        .expect("confirmable");
}

/// `open_designer` is also the escape hatch out of `generation_failed`, so
/// the same tear must repair from that source state — otherwise the recovery
/// leaves an armed gate the record does not admit to.
#[test]
fn torn_reopen_designer_from_generation_failed_repairs_to_awaiting_spec_confirmation() {
    let dir = tempfile::tempdir().unwrap();
    let mut before = fresh_app("aaaa1111");
    before.open_designer("int-1".into(), T0 + 1).unwrap();
    before.confirm_design("int-1", 0, T0 + 2).unwrap();
    before.generation_failed(T0 + 3).unwrap();
    let mut after = before.clone();
    after.open_designer("int-2".into(), T0 + 4).unwrap();
    tear(dir.path(), &before, &after, &Tear::AfterInteractions);

    let mut app = load_repaired(dir.path());
    assert_eq!(
        app.record.workflow_state,
        AppWorkflowState::AwaitingSpecConfirmation
    );
    let pending = app.interactions.pending.clone().expect("gate survives");
    assert_eq!(pending.interaction_id, "int-2");
    app.confirm_design("int-2", 0, T0 + 5).expect("confirmable");
}

#[test]
fn torn_validation_passed_repairs_to_awaiting_preview_confirmation() {
    let dir = tempfile::tempdir().unwrap();
    let mut before = fresh_app("aaaa1111");
    before.open_designer("int-1".into(), T0 + 1).unwrap();
    before.confirm_design("int-1", 0, T0 + 2).unwrap();
    before.generation_complete(T0 + 3).unwrap();
    let mut after = before.clone();
    after.validation_passed("int-p".into(), T0 + 4).unwrap();
    tear(dir.path(), &before, &after, &Tear::AfterInteractions);

    let app = load_repaired(dir.path());
    assert_eq!(
        app.record.workflow_state,
        AppWorkflowState::AwaitingPreviewConfirmation
    );
    let pending = app.interactions.pending.expect("preview gate survives");
    assert_eq!(pending.kind, AppInteractionKind::Preview);
    assert_eq!(pending.interaction_id, "int-p");
}

#[test]
fn torn_confirm_preview_repairs_to_ready() {
    let dir = tempfile::tempdir().unwrap();
    let mut before = fresh_app("aaaa1111");
    before.open_designer("int-1".into(), T0 + 1).unwrap();
    before.confirm_design("int-1", 0, T0 + 2).unwrap();
    before.generation_complete(T0 + 3).unwrap();
    before.validation_passed("int-p".into(), T0 + 4).unwrap();
    let mut after = before.clone();
    after.confirm_preview("int-p", 0, T0 + 5).unwrap();
    tear(dir.path(), &before, &after, &Tear::AfterInteractions);

    let app = load_repaired(dir.path());
    assert_eq!(app.record.workflow_state, AppWorkflowState::Ready);
    assert!(app.interactions.pending.is_none());
}

#[test]
fn torn_request_revision_from_preview_gate_repairs_to_revising() {
    let dir = tempfile::tempdir().unwrap();
    let mut before = fresh_app("aaaa1111");
    before.open_designer("int-1".into(), T0 + 1).unwrap();
    before.confirm_design("int-1", 0, T0 + 2).unwrap();
    before.generation_complete(T0 + 3).unwrap();
    before.validation_passed("int-p".into(), T0 + 4).unwrap();
    let mut after = before.clone();
    after.request_revision("darker header", T0 + 5).unwrap();
    tear(dir.path(), &before, &after, &Tear::AfterAppFiles);

    let app = load_repaired(dir.path());
    assert_eq!(app.record.workflow_state, AppWorkflowState::Revising);
    assert!(app.interactions.pending.is_none());
    let newest = app.interactions.undelivered.last().expect("prompt queued");
    assert_eq!(newest.kind, AppContinuationKind::RevisionRequested);
    assert_eq!(
        newest.payload,
        serde_json::json!({ "prompt": "darker header" })
    );
}

#[test]
fn torn_request_revision_from_ready_repairs_to_revising() {
    let dir = tempfile::tempdir().unwrap();
    let mut before = fresh_app("aaaa1111");
    before.open_designer("int-1".into(), T0 + 1).unwrap();
    before.confirm_design("int-1", 0, T0 + 2).unwrap();
    before.generation_complete(T0 + 3).unwrap();
    before.validation_passed("int-p".into(), T0 + 4).unwrap();
    before.confirm_preview("int-p", 0, T0 + 5).unwrap();
    let mut after = before.clone();
    after.request_revision("bigger buttons", T0 + 6).unwrap();
    tear(dir.path(), &before, &after, &Tear::AfterInteractions);

    let app = load_repaired(dir.path());
    assert_eq!(app.record.workflow_state, AppWorkflowState::Revising);
}

#[test]
fn torn_generation_complete_adopts_the_mirror_state() {
    let dir = tempfile::tempdir().unwrap();
    let mut before = fresh_app("aaaa1111");
    before.open_designer("int-1".into(), T0 + 1).unwrap();
    before.confirm_design("int-1", 0, T0 + 2).unwrap();
    let mut after = before.clone();
    after.generation_complete(T0 + 3).unwrap();
    tear(dir.path(), &before, &after, &Tear::AfterAppFiles);

    let app = load_repaired(dir.path());
    assert_eq!(app.record.workflow_state, AppWorkflowState::Validating);
    assert_eq!(
        app.record.updated_at_ms,
        T0 + 3,
        "mirror record adopted whole"
    );
}

#[test]
fn tampered_gate_without_evidence_is_rearmed() {
    let dir = tempfile::tempdir().unwrap();
    let mut before = fresh_app("aaaa1111");
    before.open_designer("int-1".into(), T0 + 1).unwrap();
    commit_full(dir.path(), &before);
    // Hand-tamper: the pending gate vanishes with NO continuation evidence
    // (no legal writer produces this shape).
    save_interactions(
        dir.path(),
        "aaaa1111",
        &AppInteractions {
            schema_version: APPS_SCHEMA_VERSION,
            pending: None,
            next_seq: 1,
            last_delivered_seq: 0,
            undelivered: Vec::new(),
        },
    )
    .unwrap();

    let mut app = load_repaired(dir.path());
    assert_eq!(
        app.record.workflow_state,
        AppWorkflowState::AwaitingSpecConfirmation,
        "no evidence of a consumed gate — the state stands"
    );
    let pending = app.interactions.pending.clone().expect("gate re-armed");
    assert_eq!(pending.kind, AppInteractionKind::Designer);
    assert_ne!(pending.interaction_id, "int-1", "a FRESH id is minted");
    assert_eq!(pending.revision, 0);
    // The re-armed gate is satisfiable again.
    app.confirm_design(&pending.interaction_id, 0, T0 + 9)
        .expect("confirmable");
}

#[test]
fn consistent_store_is_not_rewritten_by_load() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = fresh_app("aaaa1111");
    app.open_designer("int-1".into(), T0 + 1).unwrap();
    commit_full(dir.path(), &app);
    let index_path = dir.path().join("apps/index.json");
    let interactions_path = dir.path().join("apps/aaaa1111/interactions.json");
    let index_before = std::fs::read_to_string(&index_path).unwrap();
    let interactions_before = std::fs::read_to_string(&interactions_path).unwrap();

    let loaded = load_all(dir.path()).expect("load");
    assert_eq!(loaded, vec![app]);
    assert_eq!(
        std::fs::read_to_string(&index_path).unwrap(),
        index_before,
        "a consistent store must load read-only"
    );
    assert_eq!(
        std::fs::read_to_string(&interactions_path).unwrap(),
        interactions_before
    );
}

/// The finding's end-to-end scenario: the user's Confirm is killed between
/// the per-app batch and the index write. On relaunch the service must come
/// up in `generating` (not a bricked confirmation gate), the queued
/// `design_confirmed` continuation must deliver exactly once through the
/// startup sweep, and the phase-3 generator's `generation_complete` must be
/// accepted.
#[tokio::test]
async fn service_reload_after_torn_confirm_continues_the_flow() {
    let dir = tempfile::tempdir().unwrap();
    let mut before = fresh_app("aaaa1111");
    before.open_designer("int-1".into(), T0 + 1).unwrap();
    let mut after = before.clone();
    after.confirm_design("int-1", 0, T0 + 2).unwrap();
    tear(dir.path(), &before, &after, &Tear::AfterAppFiles);

    let sink = Arc::new(RecordingContinuationSink::new());
    let observer = Arc::new(RecordingAppEventObserver::new());
    let service = AppService::load(
        dir.path(),
        Arc::new(FixedClock::new(T0 + 10)),
        Arc::clone(&sink) as Arc<dyn ContinuationSink>,
        observer as Arc<dyn AppEventObserver>,
    )
    .await
    .expect("torn store must load");

    assert_eq!(
        service.record("aaaa1111").await.unwrap().workflow_state,
        AppWorkflowState::Generating
    );
    // Startup sweep delivers the (now truthful) continuation exactly once.
    assert_eq!(service.redeliver_all_undelivered().await.unwrap(), 1);
    let accepted = sink.accepted();
    assert_eq!(accepted.len(), 1);
    assert_eq!(accepted[0].1.kind, AppContinuationKind::DesignConfirmed);
    assert_eq!(service.redeliver_all_undelivered().await.unwrap(), 0);
    // The generator's completion is accepted — no workflow_state_invalid.
    service.generation_complete("aaaa1111").await.unwrap();
    assert_eq!(
        service.record("aaaa1111").await.unwrap().workflow_state,
        AppWorkflowState::Validating
    );
    // And the storage stays loadable/consistent for the next boot.
    drop(service);
    let reloaded = storage::load_all(dir.path()).unwrap();
    assert_eq!(
        reloaded[0].record.workflow_state,
        AppWorkflowState::Validating
    );
}
