//! Durable record of installed plugins (`<plugins>/installed_plugins.json`).
//!
//! Each successful network install (git / marketplace / `.mcpb`) records the
//! plugin under its marketplace + version so a later launch can re-discover the
//! exact cache directory (`cache/<marketplace>/<plugin>/<version>/`) instead of
//! probing. Mirrors claude-code's V2 `installed_plugins.json` shape
//! (`{ version: 2, plugins: { <marketplace>: { <plugin>: { version, added } } } }`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Schema version stamped into `installed_plugins.json` (claude-code V2).
const SCHEMA_VERSION: u32 = 2;

/// Top-level `installed_plugins.json`: `marketplace → (plugin → record)`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledPlugins {
    /// Schema version (`2`).
    #[serde(default)]
    pub version: u32,
    /// Installed records keyed by marketplace, then plugin name.
    #[serde(default)]
    pub plugins: BTreeMap<String, BTreeMap<String, InstalledPluginRecord>>,
}

impl Default for InstalledPlugins {
    fn default() -> Self {
        Self {
            version: SCHEMA_VERSION,
            plugins: BTreeMap::new(),
        }
    }
}

/// One installed-plugin record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledPluginRecord {
    /// The materialized version (`"unknown"` when the manifest has none).
    pub version: String,
    /// Install time, epoch milliseconds.
    #[serde(default)]
    pub added: u64,
}

/// Path to `installed_plugins.json` under the plugins root.
#[must_use]
pub fn path(install_dir: &Path) -> PathBuf {
    install_dir.join("installed_plugins.json")
}

/// Load the installed-plugins record (a missing / malformed file ⇒ empty V2).
pub async fn load(install_dir: &Path) -> InstalledPlugins {
    match tokio::fs::read_to_string(path(install_dir)).await {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
        Err(_) => InstalledPlugins::default(),
    }
}

/// Read-modify-write: record `plugin@marketplace` at `version` with the current
/// time. Best-effort; an I/O error is returned for the caller to log (install
/// success is not gated on the record write).
pub async fn record(
    install_dir: &Path,
    marketplace: &str,
    plugin: &str,
    version: &str,
    now_ms: u64,
) -> std::io::Result<()> {
    let mut state = load(install_dir).await;
    state.version = SCHEMA_VERSION;
    state
        .plugins
        .entry(marketplace.to_string())
        .or_default()
        .insert(
            plugin.to_string(),
            InstalledPluginRecord {
                version: version.to_string(),
                added: now_ms,
            },
        );
    let serialized = serde_json::to_string_pretty(&state)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    if let Some(parent) = path(install_dir).parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(path(install_dir), serialized).await
}

/// Current wall-clock in epoch milliseconds (host-side; the no-clock rule is for
/// workflow scripts, not the install host).
#[must_use]
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}
