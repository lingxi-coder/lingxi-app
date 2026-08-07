//! Phase-1 acceptance test (spec §J): the full designer flow with events
//! observed, then a rebuild from disk alone with exactly-once continuation
//! redelivery and seq dedup — all without any Node/runtime.

use local_apps::storage;
use local_apps::test_support::FixedClock;
use local_apps::{
    AppDesignField, AppDesignFieldOption, AppDesignFieldType, AppDesignPatch, AppDesignPatchOp,
    AppDesignStep, AppErrorCode, AppEvent, AppEventObserver, AppPlan, AppRuntimeState, AppService,
    AppWorkflowState, ContinuationSink, DesignValue, RecordingAppEventObserver,
    RecordingContinuationSink,
};
use std::path::Path;
use std::sync::Arc;

struct Harness {
    service: AppService,
    sink: Arc<RecordingContinuationSink>,
    observer: Arc<RecordingAppEventObserver>,
}

async fn harness(root: &Path) -> Harness {
    let sink = Arc::new(RecordingContinuationSink::new());
    let observer = Arc::new(RecordingAppEventObserver::new());
    let service = AppService::load(
        root,
        Arc::new(FixedClock::new(1_753_800_000_000)),
        Arc::clone(&sink) as Arc<dyn ContinuationSink>,
        Arc::clone(&observer) as Arc<dyn AppEventObserver>,
    )
    .await
    .expect("load service");
    Harness {
        service,
        sink,
        observer,
    }
}

/// Reload `h.service` from disk, keeping the SAME sink/observer `Arc`s so
/// already-recorded events survive the swap.
async fn reload(h: Harness, root: &Path) -> Harness {
    drop(h.service);
    let service = AppService::load(
        root,
        Arc::new(FixedClock::new(1_753_800_000_000)),
        Arc::clone(&h.sink) as Arc<dyn ContinuationSink>,
        Arc::clone(&h.observer) as Arc<dyn AppEventObserver>,
    )
    .await
    .expect("reload service");
    Harness {
        service,
        sink: h.sink,
        observer: h.observer,
    }
}

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

/// Task 4 wires the real LLM-driven questionnaire-authoring/planning round
/// trip through `AppService`; until then, splice a completed questionnaire
/// and a matching plan directly onto the on-disk state for `app_id` — the
/// same storage-level bypass `crash_repair.rs` uses — so this acceptance
/// test can reach `collecting_spec`/`awaiting_spec_confirmation` through
/// the public surface that exists today.
fn splice_questionnaire_and_plan(root: &Path, app_id: &str, now_ms: u64) {
    let mut apps = storage::load_all(root).expect("load for splice");
    let app = apps
        .iter_mut()
        .find(|a| a.record.id == app_id)
        .expect("app on disk");
    let epoch = app.record.llm_round;
    app.questionnaire_ready(one_step(), None, epoch, now_ms)
        .expect("fixture questionnaire is valid");
    app.draft.plan = Some(AppPlan {
        collections: Vec::new(),
        capabilities: Vec::new(),
        domains: Vec::new(),
        summary: "s".into(),
    });
    app.draft.plan_for_revision = Some(app.draft.revision);
    storage::save_app_files(root, app).expect("save spliced app");
}

fn set(field: &str, value: DesignValue) -> AppDesignPatch {
    AppDesignPatch {
        ops: vec![AppDesignPatchOp::Set {
            field_id: field.into(),
            value,
        }],
        note: None,
    }
}

fn workflow_states(events: &[AppEvent]) -> Vec<AppWorkflowState> {
    events
        .iter()
        .filter_map(|event| match event {
            AppEvent::WorkflowChanged { state, .. } => Some(*state),
            _ => None,
        })
        .collect()
}

// Spec §J mandates ONE end-to-end acceptance flow (create → designer →
// generation → preview → ready → disk rebuild → redelivery); splitting it
// would drop the cross-step invariants this test exists to pin.
#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn phase1_acceptance_designer_flow_survives_disk_rebuild() {
    let dir = tempfile::tempdir().expect("tempdir");

    // ---- Session 1: walk the full designer flow with a FAILING sink so the
    // human-gate continuations stay queued (undelivered) on disk.
    let h = harness(dir.path()).await;
    h.sink.set_fail(true);

    let record = h
        .service
        .create_app(Some("Habit Tracker"),
            "a test app",
            Some("conv-42".into()),
        )
        .await
        .expect("create app");
    let app_id = record.id.clone();

    // See `splice_questionnaire_and_plan`'s doc: advances the app from the
    // real initial `authoring_questionnaire` to `collecting_spec` with a
    // matching plan already computed, standing in for Task 4's not-yet-wired
    // LLM round trip. `h.sink`/`h.observer` survive the reload, so the
    // `AppsChanged` event `create_app` already emitted stays `events[0]`.
    splice_questionnaire_and_plan(dir.path(), &app_id, 1_753_800_000_001);
    let h = reload(h, dir.path()).await;
    h.sink.set_fail(true);

    // open_designer -> awaiting_spec_confirmation with a pending gate.
    let designer = h
        .service
        .open_designer(&app_id)
        .await
        .expect("open designer");
    assert_eq!(designer.revision, 0);

    // Conflicting update rejected: wrong expected_revision, value kept.
    let err = h
        .service
        .update_draft(
            &app_id,
            7,
            &set("title", DesignValue::ShortText("X".into())),
        )
        .await
        .expect_err("stale revision must conflict");
    assert_eq!(err.code(), AppErrorCode::RevisionConflict);

    // Valid updates.
    assert_eq!(
        h.service
            .update_draft(
                &app_id,
                0,
                &set("title", DesignValue::ShortText("Habits".into()))
            )
            .await
            .expect("update 1"),
        1
    );
    assert_eq!(
        h.service
            .update_draft(
                &app_id,
                1,
                &set(
                    "screens",
                    DesignValue::ScreenList(vec!["today".into(), "history".into()])
                )
            )
            .await
            .expect("update 2"),
        2
    );

    // Suggestion stored + applied.
    let suggestion = h
        .service
        .store_suggestion(&app_id, set("accent", DesignValue::Color("#3366ff".into())))
        .await
        .expect("store suggestion");
    assert_eq!(suggestion.based_on_revision, 2);
    assert_eq!(
        h.service
            .apply_suggestion(&app_id, &suggestion.suggestion_id, 2)
            .await
            .expect("apply suggestion"),
        3
    );

    // Gating: a guessed interaction id or a stale revision cannot confirm.
    assert_eq!(
        h.service
            .confirm_design(&app_id, "int-guessed", 3)
            .await
            .expect_err("guessed id")
            .code(),
        AppErrorCode::InteractionInvalid
    );
    assert_eq!(
        h.service
            .confirm_design(&app_id, &designer.interaction_id, 0)
            .await
            .expect_err("stale revision")
            .code(),
        AppErrorCode::RevisionConflict
    );

    // Every `update_draft` above invalidated the spliced plan (answers
    // changed, so the plan they were computed against is stale — exactly
    // what `confirm_design`'s freshness gate exists to catch). Task 4 will
    // wire a real re-plan round trip after edits; splice a fresh plan for
    // the now-current revision (3) directly, same bypass as above.
    {
        let mut apps = storage::load_all(dir.path()).expect("load for re-splice");
        let app = apps
            .iter_mut()
            .find(|a| a.record.id == app_id)
            .expect("app on disk");
        app.draft.plan = Some(AppPlan {
            collections: Vec::new(),
            capabilities: Vec::new(),
            domains: Vec::new(),
            summary: "s".into(),
        });
        app.draft.plan_for_revision = Some(app.draft.revision);
        storage::save_app_files(dir.path(), app).expect("save re-spliced app");
    }
    let h = reload(h, dir.path()).await;

    // confirm_design -> generating (continuation enqueued, delivery fails).
    h.service
        .confirm_design(&app_id, &designer.interaction_id, 3)
        .await
        .expect("confirm design");

    // generating -> validating -> awaiting_preview_confirmation.
    h.service
        .generation_complete(&app_id)
        .await
        .expect("generation complete");
    let preview = h
        .service
        .validation_passed(&app_id)
        .await
        .expect("validation passed");
    assert_eq!(preview.revision, 3);
    assert_ne!(preview.interaction_id, designer.interaction_id);

    // confirm_preview -> ready (second continuation, also undelivered).
    h.service
        .confirm_preview(&app_id, &preview.interaction_id, 3)
        .await
        .expect("confirm preview");
    assert_eq!(
        h.service
            .record(&app_id)
            .await
            .expect("record")
            .workflow_state,
        AppWorkflowState::Ready
    );

    // Events were observed along the way (delivery is async since the
    // spawned-emission refactor — flush before asserting).
    h.service.flush_events().await;
    let events = h.observer.events();
    assert!(matches!(events.first(), Some(AppEvent::AppsChanged { apps }) if apps.len() == 1));
    assert!(events.iter().any(|e| matches!(
        e,
        AppEvent::DesignerRequested { interaction_id, .. } if *interaction_id == designer.interaction_id
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        AppEvent::DesignConflict {
            expected_revision: 7,
            actual_revision: 0,
            ..
        }
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        AppEvent::DesignSuggestionAvailable { suggestion_id, .. } if *suggestion_id == suggestion.suggestion_id
    )));
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, AppEvent::DesignDraftChanged { .. }))
            .count(),
        3,
        "two updates plus the applied suggestion"
    );
    assert!(events.iter().any(|e| matches!(
        e,
        AppEvent::PreviewReady { interaction_id, revision: 3, url: None, .. }
            if *interaction_id == preview.interaction_id
    )));
    assert_eq!(
        workflow_states(&events),
        vec![
            AppWorkflowState::AwaitingSpecConfirmation,
            AppWorkflowState::Generating,
            AppWorkflowState::Validating,
            AppWorkflowState::AwaitingPreviewConfirmation,
            AppWorkflowState::Ready,
        ]
    );

    // Both continuations remain undelivered (sink failed the whole time).
    let queued = h.service.interactions(&app_id).await.expect("interactions");
    assert_eq!(queued.undelivered.len(), 2);
    assert_eq!(queued.last_delivered_seq, 0);
    assert_eq!(queued.next_seq, 3);
    assert!(h.sink.accepted().is_empty());

    // ---- Session 2: drop every in-memory handle and rebuild from disk alone.
    drop(h);
    let h2 = harness(dir.path()).await;

    let records = h2.service.list_apps().await;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].id, app_id);
    assert_eq!(records[0].name, "Habit Tracker");
    assert_eq!(records[0].workflow_state, AppWorkflowState::Ready);
    assert_eq!(records[0].conversation_id.as_deref(), Some("conv-42"));

    let draft = h2.service.draft(&app_id).await.expect("draft");
    assert_eq!(draft.revision, 3);
    assert_eq!(draft.confirmed_revision, Some(3));
    assert_eq!(
        draft.fields.get("accent"),
        Some(&DesignValue::Color("#3366ff".into()))
    );
    assert!(draft.pending_suggestion.is_none());

    let runtime = h2.service.runtime_record(&app_id).await.expect("runtime");
    assert_eq!(runtime.state, AppRuntimeState::Stopped);

    let queued = h2
        .service
        .interactions(&app_id)
        .await
        .expect("interactions");
    assert_eq!(
        queued.undelivered.iter().map(|c| c.seq).collect::<Vec<_>>(),
        vec![1, 2],
        "undelivered continuations survive the restart"
    );

    // Redelivery: exactly once, in seq order.
    assert_eq!(
        h2.service
            .redeliver_all_undelivered()
            .await
            .expect("redeliver"),
        2
    );
    let accepted = h2.sink.accepted();
    assert_eq!(
        accepted.iter().map(|(_, c)| c.seq).collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(accepted[0].1.payload, serde_json::json!({ "revision": 3 }));

    // A second sweep redelivers nothing (store-side dedup)…
    assert_eq!(
        h2.service.redeliver_all_undelivered().await.expect("sweep"),
        0
    );
    assert_eq!(h2.sink.calls().len(), 2);

    // …and even a raw seq replay is a no-op at the consumer (sink dedup).
    let replay = accepted[0].1.clone();
    h2.sink.deliver(&app_id, &replay).await.expect("replay");
    assert_eq!(h2.sink.calls().len(), 3, "replay call happens");
    assert_eq!(h2.sink.accepted().len(), 2, "replayed seq is a no-op");

    let drained = h2
        .service
        .interactions(&app_id)
        .await
        .expect("interactions");
    assert!(drained.undelivered.is_empty());
    assert_eq!(drained.last_delivered_seq, 2);
}
