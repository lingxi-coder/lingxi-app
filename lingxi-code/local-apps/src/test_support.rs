//! Deterministic helpers for local-apps tests (unit, integration, and the
//! engine's phase-1 wiring tests).

use crate::questionnaire::{
    AppDesignField, AppDesignFieldOption, AppDesignFieldType, AppDesignStep, AppPlan,
};
use crate::service::AppService;
use crate::storage;
use crate::types::AppRecord;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use traits::Clock;

/// A [`Clock`] pinned to a settable epoch-milliseconds value.
#[derive(Debug)]
pub struct FixedClock {
    ms: AtomicU64,
}

impl FixedClock {
    /// Clock reading `start_ms` epoch milliseconds until advanced.
    #[must_use]
    pub fn new(start_ms: u64) -> Self {
        Self {
            ms: AtomicU64::new(start_ms),
        }
    }

    /// Move the clock forward by `delta_ms` milliseconds.
    pub fn advance_ms(&self, delta_ms: u64) {
        self.ms.fetch_add(delta_ms, Ordering::SeqCst);
    }
}

impl Clock for FixedClock {
    fn now(&self) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(self.ms.load(Ordering::SeqCst))
    }
}

fn fixture_questionnaire() -> Vec<AppDesignStep> {
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

fn fixture_plan() -> AppPlan {
    AppPlan {
        collections: Vec::new(),
        capabilities: Vec::new(),
        domains: Vec::new(),
        summary: "s".into(),
    }
}

/// Advance a freshly created test app from `authoring_questionnaire` to
/// `collecting_spec`, standing in for Task 4's not-yet-wired
/// questionnaire-authoring LLM round trip. This crate's — and its
/// downstream engine test suites' — apps almost all predate questionnaire
/// authoring and exercise designer/generation/preview/runtime behavior that
/// has nothing to do with it, so this reaches into the service's
/// `pub(crate)` state directly — the same "construct `AppState` directly"
/// bypass `state.rs`'s own gating tests use — rather than through a public
/// API `AppService` doesn't expose yet. Returns the updated record so
/// callers can rebind their (now stale) `create_app` result rather than
/// comparing against a copy still showing `authoring_questionnaire`.
pub async fn advance_to_collecting_spec(service: &AppService, app_id: &str) -> AppRecord {
    let mut apps = service.state.lock().await;
    let app = apps
        .iter_mut()
        .find(|a| a.record.id == app_id)
        .expect("app exists");
    app.questionnaire_ready(fixture_questionnaire(), None, app.record.updated_at_ms)
        .expect("fixture questionnaire is valid");
    // Match the in-memory advance on disk too — otherwise a later
    // `AppService::load` reload sees the ORIGINAL `authoring_questionnaire`
    // record and the two diverge.
    storage::save_app_files(&service.root, app).expect("persist spliced app");
    app.record.clone()
}

/// Stamp a plan matching the CURRENT draft revision directly, standing in
/// for Task 4's not-yet-wired planning LLM round trip, so
/// `confirm_design`'s plan-freshness gate is satisfied. Every
/// `update_draft`/`apply_suggestion` since the last stamp moves the revision
/// (and `update_draft` explicitly invalidates the stamped plan) — call this
/// again immediately before each `confirm_design`.
pub async fn stamp_fresh_plan(service: &AppService, app_id: &str) {
    let mut apps = service.state.lock().await;
    let app = apps
        .iter_mut()
        .find(|a| a.record.id == app_id)
        .expect("app exists");
    app.draft.plan = Some(fixture_plan());
    app.draft.plan_for_revision = Some(app.draft.revision);
    storage::save_app_files(&service.root, app).expect("persist stamped plan");
}
