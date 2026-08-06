//! Persisted-schema compatibility goldens (spec §D; T4).
//!
//! `tests/fixtures/v1/` holds a complete checked-in on-disk store —
//! `apps/index.json` plus every per-app document (`interactions.json`,
//! `runtime.json`, `permissions.json`, `workspace/.lingxi/app.json`,
//! `workspace/.lingxi/app.manifest.json`,
//! `workspace/.lingxi/design-spec.json`) — captured at `schemaVersion` 1.
//! The tree is produced by DRIVING A REAL [`AppService`] through a legal
//! transition trace (deterministic [`FixedClock`], continuations queued
//! legally via a failing sink), so every persisted shape is one a legal
//! writer actually produces — gate/continuation coexistence, timestamps and
//! counter invariants included (the tree must pass `load_all`'s invariant
//! validation). Coverage: all nine [`DesignValue`] kinds, a pending
//! suggestion, a pending preview gate, queued continuations of three kinds,
//! a fully-populated runtime record, and a minimal app pinning every
//! omitted-optional form. Two directions are pinned:
//!
//! - **read**: [`local_apps::storage::load_all`] over the fixture tree must
//!   keep producing exactly the expected in-memory states (spelled out as
//!   literals below; only the service-minted random interaction/suggestion
//!   ids are spliced from the loaded store, after a grammar check). A renamed
//!   field, a retyped value, or a changed enum tag would break loading real
//!   user data — it fails here first.
//! - **write**: re-persisting the loaded states through the real writers
//!   ([`local_apps::storage::save_app_files`] /
//!   [`local_apps::storage::save_index`]) must reproduce every fixture
//!   document byte-for-byte. Key casing, enum tags, indentation, trailing
//!   newline and optional-field omission are all part of the persisted
//!   contract.
//!
//! Regenerating the fixtures is only legitimate together with an INTENTIONAL
//! schema change (which also means bumping
//! [`local_apps::APPS_SCHEMA_VERSION`] and providing a migration). Then:
//!
//! ```text
//! BLESS=1 cargo test -p local-apps --test serde_compat
//! ```
//!
//! (the same `BLESS=1` convention as the client-protocol wire snapshots),
//! and review the diff before committing. Re-blessing re-drives the trace,
//! so the random interaction/suggestion ids change — everything else is
//! deterministic.

use local_apps::storage::{self, save_app_files, save_index};
use local_apps::test_support::FixedClock;
use local_apps::{
    save_manifest, save_permissions, AppContinuation,
    AppContinuationKind, AppDesignDraft, AppDesignPatch, AppDesignPatchOp, AppDesignSuggestion,
    AppEventObserver, AppInteractionKind, AppInteractionRequest, AppInteractions, AppLayout,
    AppManifest, AppPermissions, AppRecord, AppRuntimeRecord, AppRuntimeState, AppService,
    AppState, AppWorkflowState, ContinuationSink, DensityLevel, DesignValue,
    NoopAppEventObserver, RecordingContinuationSink, APPS_SCHEMA_VERSION,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Directory holding the checked-in v1 store fixture.
fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v1")
}

/// `true` when invoked in regeneration mode (`BLESS=1`).
fn bless() -> bool {
    matches!(std::env::var("BLESS").as_deref(), Ok("1" | "true"))
}

/// Both tests operate on the ONE checked-in fixture store, and each scrubs
/// the runtime lock artifact its load creates — an unlink racing the sibling
/// test's lock ACQUISITION can surface as a spurious open failure, so the
/// two tests serialize on this guard (a tokio mutex: the async test holds it
/// across awaits, which a std guard must never do).
static FIXTURE_STORE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Best-effort removal of the runtime lock artifact `load_all` creates in
/// the CHECKED-IN fixture tree, so running the tests leaves the repo clean.
fn scrub_fixture_lock() {
    let _ = std::fs::remove_file(fixtures_root().join(storage::index_lock_rel()));
}

/// Base timestamp of the fixture trace (epoch ms).
const T0: u64 = 1_753_000_000_000;

/// Seed one brand-new app with a PINNED id — byte-for-byte the persistence
/// `AppService::create_app` performs (per-app files first, index entry last);
/// only the random id mint is bypassed so the fixture paths stay stable.
fn seed_app(
    root: &Path,
    existing: &mut Vec<AppState>,
    id: &str,
    name: &str,
    brief: &str,
    conversation_id: Option<String>,
) {
    let app = AppState::create(
        id.to_string(),
        name.to_string(),
        brief.to_string(),
        conversation_id,
        T0,
    );
    save_app_files(root, &app).expect("seed app files");
    // `create_app` also mints the native contract and the permission
    // document; both are pinned like the other five. Each writer creates the
    // layout skeleton itself, so no separate `initialize` is needed.
    let layout = AppLayout::new(root, id).expect("seed layout");
    save_manifest(&layout, &AppManifest::for_new_app(id, name)).expect("seed manifest");
    save_permissions(&layout, &AppPermissions::default()).expect("seed permissions");
    existing.push(app);
    let records: Vec<AppRecord> = existing.iter().map(|app| app.record.clone()).collect();
    save_index(root, &records).expect("seed index");
}

/// The nine-kind draft patch the trace applies (one op per
/// [`DesignValue`] kind; `BTreeMap` keeps the persisted field order stable).
fn all_kinds_patch() -> AppDesignPatch {
    let set = |field_id: &str, value: DesignValue| AppDesignPatchOp::Set {
        field_id: field_id.to_string(),
        value,
    };
    AppDesignPatch {
        ops: vec![
            set("accent", DesignValue::Color("#3366ff".to_string())),
            set("compact_mode", DesignValue::Boolean(true)),
            set("density", DesignValue::Density(DensityLevel::Comfortable)),
            set(
                "description",
                DesignValue::LongText("Track daily habits\nwith streaks.".to_string()),
            ),
            set(
                "features",
                DesignValue::FeatureList(vec!["streaks".to_string(), "reminders".to_string()]),
            ),
            set("layout", DesignValue::SingleChoice("grid".to_string())),
            set(
                "screens",
                DesignValue::ScreenList(vec!["today".to_string(), "history".to_string()]),
            ),
            set(
                "tags",
                DesignValue::MultipleChoice(vec!["health".to_string(), "daily".to_string()]),
            ),
            set("title", DesignValue::ShortText("Habit Tracker".to_string())),
        ],
        note: None,
    }
}

/// Drive a REAL [`AppService`] through the legal transition trace that
/// produces the fixture store under `root`. Deterministic except for the
/// service-minted interaction/suggestion ids.
// One deliberate straight-line trace: splitting it would obscure the exact
// event order the fixture pins.
#[allow(clippy::too_many_lines)]
async fn drive_canonical_store(root: &Path) {
    let mut seeded = Vec::new();
    seed_app(
        root,
        &mut seeded,
        "aaaa1111",
        "Fixture Maximal",
        "a fixture app with every design-value kind",
        Some("conv-fixture-1".to_string()),
    );
    seed_app(
        root,
        &mut seeded,
        "bbbb2222",
        "Fixture Minimal",
        "a minimal fixture app",
        None,
    );

    let clock = Arc::new(FixedClock::new(T0));
    let service_clock: Arc<FixedClock> = Arc::clone(&clock);
    let sink = Arc::new(RecordingContinuationSink::new());
    let service = AppService::load(
        root,
        service_clock,
        Arc::clone(&sink) as Arc<dyn ContinuationSink>,
        Arc::new(NoopAppEventObserver) as Arc<dyn AppEventObserver>,
    )
    .await
    .expect("load seeded store");

    let mut now = T0;
    let mut advance_to = |target: u64| {
        clock.advance_ms(target - now);
        now = target;
    };

    // ── aaaa1111: the maximal app ───────────────────────────────────────
    // One delivered continuation (seq 1) so `lastDeliveredSeq` is nonzero…
    advance_to(T0 + 100);
    let d1 = service.open_designer("aaaa1111").await.expect("open d1");
    assert!(d1.interaction_id.starts_with("int-"));
    advance_to(T0 + 150);
    service.cancel_design("aaaa1111").await.expect("cancel d1"); // seq 1 delivered
                                                                 // …then a dead sink keeps every later continuation queued (legally).
    sink.set_fail(true);
    advance_to(T0 + 200);
    let _d2 = service.open_designer("aaaa1111").await.expect("open d2");
    advance_to(T0 + 250);
    service.cancel_design("aaaa1111").await.expect("cancel d2"); // seq 2 queued
    advance_to(T0 + 300);
    service
        .update_draft("aaaa1111", 0, &all_kinds_patch())
        .await
        .expect("draft patch");
    advance_to(T0 + 350);
    let d3 = service.open_designer("aaaa1111").await.expect("open d3");
    advance_to(T0 + 400);
    service
        .confirm_design("aaaa1111", &d3.interaction_id, 1)
        .await
        .expect("confirm design"); // seq 3 queued
    advance_to(T0 + 450);
    service
        .generation_complete("aaaa1111")
        .await
        .expect("generated");
    advance_to(T0 + 500);
    service
        .validation_passed("aaaa1111")
        .await
        .expect("preview 1");
    advance_to(T0 + 550);
    service
        .request_revision("aaaa1111", "make the header darker")
        .await
        .expect("request revision"); // seq 4 queued
    advance_to(T0 + 600);
    service
        .store_suggestion(
            "aaaa1111",
            AppDesignPatch {
                ops: vec![
                    AppDesignPatchOp::Set {
                        field_id: "accent".to_string(),
                        value: DesignValue::Color("#112233".to_string()),
                    },
                    AppDesignPatchOp::Remove {
                        field_id: "tags".to_string(),
                    },
                ],
                note: Some("tone down the accent".to_string()),
            },
        )
        .await
        .expect("store suggestion");
    advance_to(T0 + 650);
    service
        .revision_ready("aaaa1111")
        .await
        .expect("revision ready");
    advance_to(T0 + 700);
    service
        .validation_passed("aaaa1111")
        .await
        .expect("preview 2");
    advance_to(T0 + 750);
    service
        .update_runtime_record(
            "aaaa1111",
            AppRuntimeState::Starting,
            Some(3111),
            Some(4242),
            None,
        )
        .await
        .expect("runtime starting");
    advance_to(T0 + 800);
    service
        .update_runtime_record(
            "aaaa1111",
            AppRuntimeState::Failed,
            None,
            Some(4242),
            Some("dev server exited with code 1".to_string()),
        )
        .await
        .expect("runtime failed");

    // ── bbbb2222: the minimal app (every optional absent) ───────────────
    advance_to(T0 + 850);
    service
        .open_designer("bbbb2222")
        .await
        .expect("open minimal");
    advance_to(T0 + 900);
    service
        .cancel_design("bbbb2222")
        .await
        .expect("cancel minimal"); // seq 1 queued
}

/// The in-memory states the fixture tree must load to, spelled out as
/// literals. Only the service-minted random ids (the pending preview gate's
/// `interaction_id`, the pending suggestion's `suggestion_id`) are spliced
/// from `loaded` — after asserting they match the id grammar — so everything
/// else stays pinned independently of the loader.
// One long literal on purpose: the golden must spell out every field of every
// document — splitting it into builders would hide exactly what is pinned.
#[allow(clippy::too_many_lines)]
fn expected_states(loaded: &[AppState]) -> Vec<AppState> {
    assert_eq!(loaded.len(), 2, "fixture store holds exactly two apps");
    let pending_gate = loaded[0]
        .interactions
        .pending
        .as_ref()
        .expect("the maximal app must hold its pending preview gate");
    assert!(
        pending_gate.interaction_id.starts_with("int-") && pending_gate.interaction_id.len() == 16,
        "gate id must match the service mint grammar: {:?}",
        pending_gate.interaction_id
    );
    let suggestion_id = &loaded[0]
        .draft
        .pending_suggestion
        .as_ref()
        .expect("the maximal app must hold its pending suggestion")
        .suggestion_id;
    assert!(
        suggestion_id.starts_with("sugg-") && suggestion_id.len() == 17,
        "suggestion id must match the service mint grammar: {suggestion_id:?}"
    );

    let maximal_fields: BTreeMap<String, DesignValue> = all_kinds_patch()
        .ops
        .into_iter()
        .map(|op| match op {
            AppDesignPatchOp::Set { field_id, value } => (field_id, value),
            AppDesignPatchOp::Remove { .. } => unreachable!("patch only sets"),
        })
        .collect();

    let maximal = AppState {
        record: AppRecord {
            id: "aaaa1111".to_string(),
            name: "Fixture Maximal".to_string(),
            brief: "a fixture app with every design-value kind".to_string(),
            created_at_ms: T0,
            updated_at_ms: T0 + 700,
            workflow_state: AppWorkflowState::AwaitingPreviewConfirmation,
            conversation_id: Some("conv-fixture-1".to_string()),
            workspace_rel: "apps/aaaa1111/workspace".to_string(),
        },
        draft: AppDesignDraft {
            schema_version: APPS_SCHEMA_VERSION,
            revision: 1,
            questionnaire: Vec::new(),
            fields: maximal_fields,
            plan: None,
            plan_for_revision: None,
            pending_suggestion: Some(AppDesignSuggestion {
                suggestion_id: suggestion_id.clone(),
                patch: AppDesignPatch {
                    ops: vec![
                        AppDesignPatchOp::Set {
                            field_id: "accent".to_string(),
                            value: DesignValue::Color("#112233".to_string()),
                        },
                        AppDesignPatchOp::Remove {
                            field_id: "tags".to_string(),
                        },
                    ],
                    note: Some("tone down the accent".to_string()),
                },
                based_on_revision: 1,
            }),
            confirmed_revision: Some(1),
        },
        interactions: AppInteractions {
            schema_version: APPS_SCHEMA_VERSION,
            pending: Some(AppInteractionRequest {
                interaction_id: pending_gate.interaction_id.clone(),
                app_id: "aaaa1111".to_string(),
                kind: AppInteractionKind::Preview,
                revision: 1,
                created_at_ms: T0 + 700,
            }),
            next_seq: 5,
            last_delivered_seq: 1,
            undelivered: vec![
                AppContinuation {
                    seq: 2,
                    app_id: "aaaa1111".to_string(),
                    kind: AppContinuationKind::DesignCancelled,
                    payload: serde_json::json!({}),
                    created_at_ms: T0 + 250,
                },
                AppContinuation {
                    seq: 3,
                    app_id: "aaaa1111".to_string(),
                    kind: AppContinuationKind::DesignConfirmed,
                    payload: serde_json::json!({ "revision": 1 }),
                    created_at_ms: T0 + 400,
                },
                AppContinuation {
                    seq: 4,
                    app_id: "aaaa1111".to_string(),
                    kind: AppContinuationKind::RevisionRequested,
                    payload: serde_json::json!({ "prompt": "make the header darker" }),
                    created_at_ms: T0 + 550,
                },
            ],
        },
        runtime: AppRuntimeRecord {
            schema_version: APPS_SCHEMA_VERSION,
            app_id: "aaaa1111".to_string(),
            state: AppRuntimeState::Failed,
            mode: None,
            port: Some(3111),
            pid: Some(4242),
            last_error: Some("dev server exited with code 1".to_string()),
            updated_at_ms: T0 + 800,
        },
    };

    let minimal = AppState {
        record: AppRecord {
            id: "bbbb2222".to_string(),
            name: "Fixture Minimal".to_string(),
            brief: "a minimal fixture app".to_string(),
            created_at_ms: T0,
            updated_at_ms: T0 + 900,
            workflow_state: AppWorkflowState::CollectingSpec,
            conversation_id: None,
            workspace_rel: "apps/bbbb2222/workspace".to_string(),
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
        interactions: AppInteractions {
            schema_version: APPS_SCHEMA_VERSION,
            pending: None,
            next_seq: 2,
            last_delivered_seq: 0,
            undelivered: vec![AppContinuation {
                seq: 1,
                app_id: "bbbb2222".to_string(),
                kind: AppContinuationKind::DesignCancelled,
                payload: serde_json::json!({}),
                created_at_ms: T0 + 900,
            }],
        },
        runtime: AppRuntimeRecord {
            schema_version: APPS_SCHEMA_VERSION,
            app_id: "bbbb2222".to_string(),
            state: AppRuntimeState::Stopped,
            mode: None,
            port: None,
            pid: None,
            last_error: None,
            updated_at_ms: T0,
        },
    };

    vec![maximal, minimal]
}

/// Persist `states` under `root` through the REAL writer path (per-app files
/// first, index last — the same order the service commits in). The manifest
/// and permission documents are not part of [`AppState`], so each is minted
/// from the same PRODUCER `create_app` uses ([`seed_app`] does the same) —
/// copying them out of the fixture instead would compare the fixture against
/// a round-trip of itself for exactly the two documents this pins.
fn write_store(root: &Path, states: &[AppState]) {
    for app in states {
        save_app_files(root, app).expect("save app files");
        let layout = AppLayout::new(root, app.record.id.clone()).expect("target layout");
        save_manifest(
            &layout,
            &AppManifest::for_new_app(&app.record.id, &app.record.name),
        )
        .expect("save manifest");
        save_permissions(&layout, &AppPermissions::default()).expect("save permissions");
    }
    let records: Vec<AppRecord> = states.iter().map(|app| app.record.clone()).collect();
    save_index(root, &records).expect("save index");
}

/// Every file under `root`, as sorted root-relative paths. The advisory
/// index lock (`apps/index.lock`) is excluded: it is a runtime artifact
/// created by merely LOADING a store (finding 9), not part of the persisted
/// document contract these goldens pin.
fn walk_files(root: &Path) -> Vec<PathBuf> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
        let entries = std::fs::read_dir(dir)
            .unwrap_or_else(|error| panic!("read_dir {}: {error}", dir.display()));
        for entry in entries {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                out.push(
                    path.strip_prefix(root)
                        .expect("walked path is under root")
                        .to_path_buf(),
                );
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.retain(|rel| *rel != storage::index_lock_rel());
    out.sort();
    out
}

/// READ direction: the checked-in v1 tree must keep loading to exactly the
/// expected states through the loader [`local_apps::AppService`] uses —
/// which also proves the fixture passes `load_all`'s continuation-invariant
/// validation and needs neither torn-commit repair nor runtime
/// reconciliation.
#[test]
fn fixture_store_loads_to_the_canonical_states() {
    if bless() {
        return; // the sibling test is rewriting the tree
    }
    let _store = FIXTURE_STORE.blocking_lock();
    let loaded = storage::load_all(&fixtures_root())
        .expect("the checked-in v1 fixture store must load without a migration");
    scrub_fixture_lock();
    let expected = expected_states(&loaded);
    assert_eq!(
        loaded, expected,
        "parsing the v1 fixtures drifted — old on-disk stores would load wrong"
    );
}

/// WRITE direction: re-persisting the loaded fixture states through the real
/// writers must reproduce every fixture document byte-for-byte (and produce
/// no extra / missing files). Under `BLESS=1` the tree is regenerated by
/// driving the real service trace instead.
#[tokio::test]
async fn writers_reproduce_the_fixture_bytes_exactly() {
    let fixtures = fixtures_root();
    let _store = FIXTURE_STORE.lock().await;

    if bless() {
        let apps_dir = fixtures.join(storage::APPS_DIR);
        if apps_dir.exists() {
            std::fs::remove_dir_all(&apps_dir).expect("clear stale fixtures");
        }
        std::fs::create_dir_all(&fixtures).expect("create fixtures root");
        drive_canonical_store(&fixtures).await;
        scrub_fixture_lock();
        return;
    }

    let states = storage::load_all(&fixtures).expect("fixtures load");
    scrub_fixture_lock();
    // Belt and braces: the states we re-serialize are the pinned ones.
    assert_eq!(states, expected_states(&states));

    let tmp = tempfile::tempdir().expect("tempdir");
    write_store(tmp.path(), &states);

    let written_files = walk_files(tmp.path());
    let fixture_files = walk_files(&fixtures);
    assert_eq!(
        written_files, fixture_files,
        "the writers and the checked-in fixture tree must contain the same files; \
         if a document was intentionally added/removed, re-bless with \
         `BLESS=1 cargo test -p local-apps --test serde_compat`"
    );

    let mut failures = Vec::new();
    for rel in &fixture_files {
        let want = std::fs::read_to_string(fixtures.join(rel))
            .unwrap_or_else(|error| panic!("read fixture {}: {error}", rel.display()));
        let got = std::fs::read_to_string(tmp.path().join(rel))
            .unwrap_or_else(|error| panic!("read written {}: {error}", rel.display()));
        if got != want {
            failures.push(format!(
                "`{}` drifted from the persisted v1 contract.\n--- fixture ---\n{want}\
                 --- writer ---\n{got}",
                rel.display()
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} persisted document(s) drifted:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
