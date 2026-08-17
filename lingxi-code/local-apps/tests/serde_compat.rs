//! Persisted-schema compatibility goldens (spec §D).
//!
//! `tests/fixtures/v1/` holds a complete checked-in on-disk store —
//! `apps/index.json` plus every per-app document (`runtime.json`,
//! `permissions.json`, `workspace/.lingxi/app.json`,
//! `workspace/.lingxi/app.manifest.json`) — captured at `schemaVersion` 1.
//! The tree is produced by DRIVING A REAL [`AppService`] through a legal
//! trace (deterministic [`FixedClock`]), so every persisted shape is one a
//! legal writer actually produces. Coverage: a `ready` app with a fully
//! populated runtime record and a conversation id, and a minimal
//! `git_enabled: false` draft app pinning every omitted-optional form. Two
//! directions are pinned:
//!
//! - **read**: [`local_apps::storage::load_all`] over the fixture tree must
//!   keep producing exactly the expected in-memory states (spelled out as
//!   literals below). A renamed field, a retyped value, or a changed enum
//!   tag would break loading real user data — it fails here first.
//! - **write**: re-persisting the loaded states through the real writers
//!   ([`local_apps::storage::save_app_files`] /
//!   [`local_apps::storage::save_index`]) must reproduce every fixture
//!   document byte-for-byte. Key casing, enum tags, indentation, trailing
//!   newline and optional-field omission are all part of the persisted
//!   contract.
//!
//! A third test pins the LEGACY migration: a pipeline-era store (mid-pipeline
//! `workflowState`, stale `interactions.json`/`design-spec.json`) loads with
//! the state collapsed to `draft` and the stale documents left untouched.
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
//! which is fully deterministic.

use local_apps::storage::{self, save_app_files, save_index};
use local_apps::test_support::FixedClock;
use local_apps::{
    save_manifest, save_permissions, AppEventObserver, AppLayout, AppManifest, AppPermissions,
    AppRecord, AppRuntimeMode, AppRuntimeRecord, AppRuntimeState, AppService, AppState,
    AppWorkflowState, NoopAppEventObserver, APPS_SCHEMA_VERSION,
};
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

/// The fixture tests operate on the ONE checked-in fixture store, and each
/// scrubs the runtime lock artifact its load creates — an unlink racing the
/// sibling test's lock ACQUISITION can surface as a spurious open failure,
/// so the tests serialize on this guard (a tokio mutex: the async test holds
/// it across awaits, which a std guard must never do).
static FIXTURE_STORE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Best-effort removal of the runtime lock artifact `load_all` creates in
/// the CHECKED-IN fixture tree, so running the tests leaves the repo clean.
fn scrub_fixture_lock() {
    let _ = std::fs::remove_file(fixtures_root().join(storage::index_lock_rel()));
}

/// Base timestamp of the fixture trace (epoch ms).
const T0: u64 = 1_753_000_000_000;

/// Seed one brand-new app with a PINNED id — byte-for-byte the persistence
/// `AppService::create_app_with_git` performs (per-app files first, index
/// entry last); only the random id mint is bypassed so the fixture paths
/// stay stable.
fn seed_app(
    root: &Path,
    existing: &mut Vec<AppState>,
    id: &str,
    name: &str,
    brief: &str,
    conversation_id: Option<String>,
    git_enabled: bool,
) {
    let app = AppState::create_with_git(
        id.to_string(),
        name.to_string(),
        brief.to_string(),
        conversation_id,
        git_enabled,
        T0,
    );
    save_app_files(root, &app).expect("seed app files");
    // `create_app_with_git` also mints the native contract and the
    // permission document; both are pinned like the other three. Each
    // writer creates the layout skeleton itself, so no separate
    // `initialize` is needed.
    let layout = AppLayout::new(root, id).expect("seed layout");
    save_manifest(&layout, &AppManifest::for_new_app(id, name)).expect("seed manifest");
    save_permissions(&layout, &AppPermissions::default()).expect("seed permissions");
    existing.push(app);
    let records: Vec<AppRecord> = existing.iter().map(|app| app.record.clone()).collect();
    save_index(root, &records).expect("seed index");
}

/// Drive a REAL [`AppService`] through the legal trace that produces the
/// fixture store under `root`. Fully deterministic.
async fn drive_canonical_store(root: &Path) {
    let mut seeded = Vec::new();
    seed_app(
        root,
        &mut seeded,
        "aaaa1111",
        "Fixture Ready",
        "a ready fixture app with a full runtime record",
        Some("conv-fixture-1".to_string()),
        true,
    );
    seed_app(
        root,
        &mut seeded,
        "bbbb2222",
        "Fixture Minimal",
        "a minimal fixture app without git",
        None,
        false,
    );

    let clock = Arc::new(FixedClock::new(T0));
    let service_clock: Arc<FixedClock> = Arc::clone(&clock);
    let service = AppService::load(
        root,
        service_clock,
        Arc::new(NoopAppEventObserver) as Arc<dyn AppEventObserver>,
    )
    .await
    .expect("load seeded store");

    let mut now = T0;
    let mut advance_to = |target: u64| {
        clock.advance_ms(target - now);
        now = target;
    };

    // ── aaaa1111: the host stamps it ready after a successful build… ────
    advance_to(T0 + 100);
    service.mark_ready("aaaa1111").await.expect("mark ready");
    // …the distribution picks a runtime mode…
    advance_to(T0 + 200);
    service
        .set_runtime_mode("aaaa1111", AppRuntimeMode::StaticExport)
        .await
        .expect("set runtime mode");
    // …and a start/fail cycle populates every runtime field.
    advance_to(T0 + 300);
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
    advance_to(T0 + 400);
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
    service.flush_events().await;
}

/// The in-memory states the fixture tree must load to, spelled out as
/// literals — nothing is spliced from the loaded store, the whole shape is
/// pinned independently of the loader.
fn expected_states() -> Vec<AppState> {
    let ready = AppState {
        record: AppRecord {
            id: "aaaa1111".to_string(),
            name: "Fixture Ready".to_string(),
            brief: "a ready fixture app with a full runtime record".to_string(),
            workflow_model: None,
            git_enabled: true,
            created_at_ms: T0,
            updated_at_ms: T0 + 100,
            workflow_state: AppWorkflowState::Ready,
            conversation_id: Some("conv-fixture-1".to_string()),
            init_session_id: None,
            workspace_rel: "apps/aaaa1111/workspace".to_string(),
        },
        runtime: AppRuntimeRecord {
            schema_version: APPS_SCHEMA_VERSION,
            app_id: "aaaa1111".to_string(),
            state: AppRuntimeState::Failed,
            mode: Some(AppRuntimeMode::StaticExport),
            port: Some(3111),
            pid: Some(4242),
            last_error: Some("dev server exited with code 1".to_string()),
            updated_at_ms: T0 + 400,
        },
    };

    let minimal = AppState {
        record: AppRecord {
            id: "bbbb2222".to_string(),
            name: "Fixture Minimal".to_string(),
            brief: "a minimal fixture app without git".to_string(),
            workflow_model: None,
            git_enabled: false,
            created_at_ms: T0,
            updated_at_ms: T0,
            workflow_state: AppWorkflowState::Draft,
            conversation_id: None,
            init_session_id: None,
            workspace_rel: "apps/bbbb2222/workspace".to_string(),
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

    vec![ready, minimal]
}

/// Persist `states` under `root` through the REAL writer path (per-app files
/// first, index last — the same order the service commits in). The manifest
/// and permission documents are not part of [`AppState`], so each is minted
/// from the same PRODUCER `create_app_with_git` uses ([`seed_app`] does the
/// same) — copying them out of the fixture instead would compare the fixture
/// against a round-trip of itself for exactly the two documents this pins.
fn write_store(root: &Path, states: &[AppState]) {
    for app in states {
        save_app_files(root, app).expect("save app files");
        let layout = AppLayout::new(root, app.record.id.clone()).expect("target layout");
        let mut manifest = AppManifest::for_new_app(&app.record.id, &app.record.name);
        // Reproduce the checked-in legacy fixture: a v1 manifest predates the
        // runtimeApiVersion field and must remain byte-stable for migration
        // compatibility tests.
        manifest.runtime_api_version = 1;
        save_manifest(&layout, &manifest).expect("save manifest");
        save_permissions(&layout, &AppPermissions::default()).expect("save permissions");
    }
    let records: Vec<AppRecord> = states.iter().map(|app| app.record.clone()).collect();
    save_index(root, &records).expect("save index");
}

/// Every file under `root`, as sorted root-relative paths. The advisory
/// index lock (`apps/index.lock`) is excluded: it is a runtime artifact
/// created by merely LOADING a store, not part of the persisted document
/// contract these goldens pin.
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
/// which also proves the fixture needs neither torn-commit repair nor
/// runtime reconciliation.
#[test]
fn fixture_store_loads_to_the_canonical_states() {
    if bless() {
        return; // the sibling test is rewriting the tree
    }
    let _store = FIXTURE_STORE.blocking_lock();
    let loaded = storage::load_all(&fixtures_root())
        .expect("the checked-in v1 fixture store must load without a migration");
    scrub_fixture_lock();
    assert_eq!(
        loaded,
        expected_states(),
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
    assert_eq!(states, expected_states());

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

/// LEGACY migration: a pipeline-era store — a mid-pipeline `workflowState`
/// plus the pipeline's own documents (`interactions.json`,
/// `design-spec.json`) — must load with the state collapsed to `draft` (the
/// serde aliases) and the stale documents left on disk untouched (the loader
/// never reads them; nothing deletes them).
#[test]
fn a_legacy_pipeline_store_loads_as_draft_and_ignores_stale_docs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let app_dir = root.join("apps/legacy01");
    std::fs::create_dir_all(app_dir.join("workspace/.lingxi")).expect("mkdir");

    // A legacy record mid-pipeline, exactly as a pre-v3 build persisted it.
    let legacy_record = r#"{
    "id": "legacy01",
    "name": "Legacy Habits",
    "brief": "a legacy pipeline app",
    "gitEnabled": true,
    "createdAtMs": 1753000000000,
    "updatedAtMs": 1753000000700,
    "workflowState": "awaiting_preview_confirmation",
    "conversationId": "conv-legacy-1",
    "workspaceRel": "apps/legacy01/workspace"
  }"#;
    std::fs::write(
        root.join("apps/index.json"),
        format!(
            "{{\n  \"schemaVersion\": 1,\n  \"apps\": [\n    {}\n  ]\n}}\n",
            legacy_record.trim()
        ),
    )
    .expect("write index");
    std::fs::write(
        app_dir.join("workspace/.lingxi/app.json"),
        format!(
            "{{\n  \"schemaVersion\": 1,\n  \"app\": {}\n}}\n",
            legacy_record.trim()
        ),
    )
    .expect("write mirror");
    std::fs::write(
        app_dir.join("runtime.json"),
        "{\n  \"schemaVersion\": 1,\n  \"appId\": \"legacy01\",\n  \"state\": \"stopped\",\n  \"updatedAtMs\": 1753000000000\n}\n",
    )
    .expect("write runtime");

    // Plausible legacy pipeline documents (shapes copied from the pre-v3
    // fixture store): a pending preview gate + queued continuations, and a
    // design draft with answers and a pending suggestion.
    let stale_interactions = r#"{
  "schemaVersion": 1,
  "pending": {
    "interactionId": "int-7df09b4cb739",
    "appId": "legacy01",
    "kind": "preview",
    "revision": 1,
    "createdAtMs": 1753000000700
  },
  "nextSeq": 5,
  "lastDeliveredSeq": 1,
  "undelivered": [
    {
      "seq": 3,
      "appId": "legacy01",
      "kind": "design_confirmed",
      "payload": {
        "revision": 1
      },
      "createdAtMs": 1753000000400
    },
    {
      "seq": 4,
      "appId": "legacy01",
      "kind": "revision_requested",
      "payload": {
        "prompt": "make the header darker"
      },
      "createdAtMs": 1753000000550
    }
  ]
}
"#;
    let stale_design_spec = r##"{
  "schemaVersion": 1,
  "revision": 1,
  "questionnaire": [],
  "fields": {
    "accent": {
      "kind": "color",
      "value": "#3366ff"
    },
    "title": {
      "kind": "short_text",
      "value": "Habit Tracker"
    }
  },
  "pendingSuggestion": {
    "suggestionId": "sugg-a87290bde485",
    "patch": {
      "ops": [
        {
          "op": "set",
          "fieldId": "accent",
          "value": {
            "kind": "color",
            "value": "#112233"
          }
        }
      ],
      "note": "tone down the accent"
    },
    "basedOnRevision": 1
  },
  "confirmedRevision": 1
}
"##;
    let interactions_path = app_dir.join("interactions.json");
    let design_spec_path = app_dir.join("workspace/.lingxi/design-spec.json");
    std::fs::write(&interactions_path, stale_interactions).expect("write interactions");
    std::fs::write(&design_spec_path, stale_design_spec).expect("write design spec");

    let index_before = std::fs::read_to_string(root.join("apps/index.json")).unwrap();
    let mirror_before =
        std::fs::read_to_string(app_dir.join("workspace/.lingxi/app.json")).unwrap();

    let loaded = storage::load_all(root).expect("a legacy pipeline store must load");
    assert_eq!(loaded.len(), 1);
    let app = &loaded[0];
    assert_eq!(app.record.id, "legacy01");
    assert_eq!(
        app.record.workflow_state,
        AppWorkflowState::Draft,
        "every mid-pipeline legacy state collapses to draft"
    );
    assert_eq!(app.record.conversation_id.as_deref(), Some("conv-legacy-1"));
    assert!(app.record.git_enabled);
    assert_eq!(app.runtime.state, AppRuntimeState::Stopped);

    // The stale pipeline documents were neither read-repaired nor deleted.
    assert_eq!(
        std::fs::read_to_string(&interactions_path).unwrap(),
        stale_interactions,
        "interactions.json must be left byte-for-byte untouched"
    );
    assert_eq!(
        std::fs::read_to_string(&design_spec_path).unwrap(),
        stale_design_spec,
        "design-spec.json must be left byte-for-byte untouched"
    );
    // And the load needed no repair rewrite either (index and mirror agree
    // once both parse to `draft`), so even those stay untouched.
    assert_eq!(
        std::fs::read_to_string(root.join("apps/index.json")).unwrap(),
        index_before
    );
    assert_eq!(
        std::fs::read_to_string(app_dir.join("workspace/.lingxi/app.json")).unwrap(),
        mirror_before
    );
}
