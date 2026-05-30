//! Feature-flag client with cache + background refresh.
//!
//! See spec §26.4. A pluggable [`FeatureFlagsFetcher`] (`Statsig`,
//! `LaunchDarkly`, file-backed) supplies values; the client caches them and
//! refreshes on a fixed interval via the [`RuntimeSpawner`] trait so engine
//! code never spawns tasks directly.

use lingxi_traits::{RuntimeError, RuntimeSpawner};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

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
