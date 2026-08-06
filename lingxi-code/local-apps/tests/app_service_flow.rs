//! Phase-1 acceptance test (spec §J): the full designer flow with events
//! observed, then a rebuild from disk alone with exactly-once continuation
//! redelivery and seq dedup — all without any Node/runtime.

use local_apps::test_support::FixedClock;
use local_apps::{
    AppDesignPatch, AppDesignPatchOp, AppErrorCode, AppEvent, AppEventObserver, AppRuntimeState,
    AppService, AppWorkflowState, ContinuationSink, DesignValue,
    RecordingAppEventObserver, RecordingContinuationSink,
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
        .create_app(
            "Habit Tracker",
            "a test app",
            Some("conv-42".into()),
        )
        .await
        .expect("create app");
    let app_id = record.id.clone();

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
