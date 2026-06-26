//! Feature-flag client with cache + background refresh.
//!
//! See spec §26.4. A pluggable [`FeatureFlagsFetcher`] (`Statsig`,
//! `LaunchDarkly`, file-backed) supplies values; the client caches them and
//! refreshes on a fixed interval via the [`RuntimeSpawner`] trait so engine
//! code never spawns tasks directly.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::RwLock as StdRwLock;
use std::time::Duration;
use tokio::sync::RwLock;
use traits::{RuntimeError, RuntimeSpawner};

// ── Synchronous GrowthBook-style flag reader (binary `nt`) ───────────────────
//
// Binary `nt(key,default)` (cc_all.txt:504507) is a SYNC read of a cached flag
// snapshot with default-on-miss: it checks an override layer `ROt()` (the static
// `Uvi` map, normally null), then — if GrowthBook is enabled (`$4()`) — reads
// `Dt().cachedGrowthBookFeatures?.[key]`, returning the passed `default` whenever
// the flag is absent or GrowthBook is disabled. There is NO env layer inside `nt`
// itself; env overrides (e.g. `LINGXI_LOOP_PERSISTENT`) are applied by the
// CALLER (`YIn`/`iKi`), not here.
//
// The port mirrors this with two process-global maps:
//   - `FLAG_SNAPSHOT`  = `Dt().cachedGrowthBookFeatures` — populated by the async
//     refresh loop if/when a real fetcher is wired; EMPTY by default (no fetcher
//     in prod), so `flag_bool` returns its `default` — byte-identical to the
//     shipped binary (GrowthBook-absent / flag-at-default).
//   - `FLAG_TEST_OVERRIDE` = `ROt()`'s `Uvi` — a test-only override layer so
//     tests can flip a flag without env vars; checked FIRST, exactly like `nt`.

fn flag_snapshot() -> &'static StdRwLock<HashMap<String, FeatureValue>> {
    static SNAP: OnceLock<StdRwLock<HashMap<String, FeatureValue>>> = OnceLock::new();
    SNAP.get_or_init(|| StdRwLock::new(HashMap::new()))
}

fn flag_test_override() -> &'static StdRwLock<HashMap<String, bool>> {
    static OVR: OnceLock<StdRwLock<HashMap<String, bool>>> = OnceLock::new();
    OVR.get_or_init(|| StdRwLock::new(HashMap::new()))
}

/// `nt(key, default)` (cc_all.txt:504507) — synchronous boolean flag read with
/// default-on-miss. Checks the test-override layer first (binary `ROt()`/`Uvi`),
/// then the cached snapshot (binary `cachedGrowthBookFeatures`), else returns
/// `default`. With no fetcher wired (the prod default) the snapshot is empty and
/// every read returns `default` — matching the shipped binary's GrowthBook-absent
/// behavior.
#[must_use]
pub fn flag_bool(key: &str, default: bool) -> bool {
    if let Some(v) = flag_test_override().read().unwrap().get(key) {
        return *v;
    }
    match flag_snapshot().read().unwrap().get(key) {
        Some(FeatureValue::Bool(b)) => *b,
        _ => default,
    }
}

/// Test-only: set a flag in the override layer (binary `ROt()`/`Uvi`). Lets tests
/// exercise flag-gated paths without process-wide env vars.
pub fn test_set_flag(key: &str, value: bool) {
    flag_test_override()
        .write()
        .unwrap()
        .insert(key.to_string(), value);
}

/// Test-only: clear a flag from the override layer.
pub fn test_clear_flag(key: &str) {
    flag_test_override().write().unwrap().remove(key);
}

/// Polymorphic feature-flag value.
///
/// Encoded with `#[serde(untagged)]` to match raw JSON shapes from
/// `Statsig` / `LaunchDarkly` responses without tag wrappers.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum FeatureValue {
    /// Boolean flag.
    Bool(bool),
    /// Numeric flag (any JSON number).
    Number(f64),
    /// String flag (e.g. variant name).
    String(String),
    /// Arbitrary JSON for structured config flags.
    Json(serde_json::Value),
}

/// Fetcher abstraction backing [`FeatureFlagsClient`].
///
/// Implementations call out to whatever provider (Statsig, etc.) and return a
/// full snapshot of flag values. Errors are reported as `Result<_, String>`
/// to keep the trait provider-agnostic.
#[async_trait::async_trait]
pub trait FeatureFlagsFetcher: Send + Sync {
    /// Return a fresh snapshot of every flag known to the provider.
    async fn fetch(&self) -> Result<HashMap<String, FeatureValue>, String>;
}

/// Cached feature-flag client with a background refresh loop.
///
/// Construct via [`Self::new`], then call [`Self::start_refresh_loop`] once
/// during platform init to begin periodic refresh.
pub struct FeatureFlagsClient {
    cache: RwLock<HashMap<String, FeatureValue>>,
    cache_ttl: Duration,
    fetcher: Arc<dyn FeatureFlagsFetcher>,
    runtime: Arc<dyn RuntimeSpawner>,
}

impl FeatureFlagsClient {
    /// Construct a client with a 5-minute refresh cadence.
    #[must_use]
    pub fn new(fetcher: Arc<dyn FeatureFlagsFetcher>, runtime: Arc<dyn RuntimeSpawner>) -> Self {
        Self {
            cache: RwLock::new(HashMap::new()),
            cache_ttl: Duration::from_secs(300),
            fetcher,
            runtime,
        }
    }

    /// Look up a boolean flag, returning `default` on miss or type mismatch.
    pub async fn get_bool(&self, key: &str, default: bool) -> bool {
        match self.cache.read().await.get(key) {
            Some(FeatureValue::Bool(b)) => *b,
            _ => default,
        }
    }

    /// Spawn the background refresh loop. Returns once the spawn has been
    /// registered with the runtime; the loop then runs until the runtime
    /// shuts down.
    pub async fn start_refresh_loop(self: Arc<Self>) -> Result<(), RuntimeError> {
        let me = self.clone();
        self.runtime
            .spawn(
                "feature-flags-refresh",
                Box::pin(async move {
                    loop {
                        if let Ok(values) = me.fetcher.fetch().await {
                            // Mirror into the sync snapshot so `flag_bool` (binary
                            // `nt`) sees live values once a real fetcher is wired.
                            if let Ok(mut snap) = flag_snapshot().write() {
                                *snap = values.clone();
                            }
                            *me.cache.write().await = values;
                        }
                        tokio::time::sleep(me.cache_ttl).await;
                    }
                }),
            )
            .await?;
        Ok(())
    }
}
