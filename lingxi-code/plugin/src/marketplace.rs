//! Marketplace catalog resolution + plugin materialization (Stage 2 install).
//!
//! A marketplace is a git repo whose `.lingxi-plugin/marketplace.json` lists
//! installable plugins. Installing `plugin@marketplace` clones the catalog into
//! `<plugins>/marketplaces/<name>/`, finds the entry, and materializes that
//! plugin's tree into the versioned cache the discovery loader resolves.
//!
//! Field names mirror claude-code's `marketplace.json` verbatim. HTTP-URL
//! catalogs, `installed_plugins.json` persistence, and non-path plugin sources
//! (git/url sub-sources) are follow-up work; this wires the canonical git-repo,
//! path-based-entry path.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The `.lingxi-plugin/marketplace.json` catalog.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MarketplaceIndex {
    /// Marketplace name.
    pub name: String,
    /// Installable plugins.
    #[serde(default)]
    pub plugins: Vec<MarketplacePluginEntry>,
}

/// One plugin entry in a marketplace catalog.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MarketplacePluginEntry {
    /// Plugin name (the `<plugin>` in `plugin@marketplace`).
    pub name: String,
    /// Declared version (informational; the real version comes from the
    /// plugin's own `plugin.json` at materialize time).
    #[serde(default)]
    pub version: Option<String>,
    /// Plugin root within the marketplace repo (a.k.a. `pluginRoot`). `None` ⇒
    /// the plugin lives at the repo root.
    #[serde(default, alias = "pluginRoot")]
    pub path: Option<String>,
    /// A plugin hosted OUTSIDE the marketplace repo (git/url sub-source). Not
    /// yet materialized (follow-up); flagged so the arm can error clearly.
    #[serde(default)]
    pub external: bool,
}

/// Errors resolving / materializing from a marketplace (the string is the
/// byte-faithful `marketplace:`-error detail).
pub type MarketplaceError = String;

/// Resolves marketplace catalogs and materializes plugins from them.
pub struct MarketplaceManager {
    /// The plugins root (`~/.lingxi/plugins`).
    install_dir: PathBuf,
}

impl MarketplaceManager {
    /// Construct over the plugins root directory.
    #[must_use]
    pub fn new(install_dir: PathBuf) -> Self {
        Self { install_dir }
    }

    /// Clone (or refresh) the marketplace git repo at `url` into
    /// `marketplaces/<name>/` and parse its `.lingxi-plugin/marketplace.json`.
    /// Returns the parsed index + the clone directory.
    pub async fn resolve_index_via_git(
        &self,
        url: &str,
        name: &str,
    ) -> Result<(MarketplaceIndex, PathBuf), MarketplaceError> {
        let clone_dir = self
            .install_dir
            .join("marketplaces")
            .join(crate::discovery::sanitize_segment(name, false));
        if clone_dir.exists() {
            tokio::fs::remove_dir_all(&clone_dir).await.ok();
        }
        if let Some(parent) = clone_dir.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("Failed to clone marketplace repository: {e}"))?;
        }
        let (u, cd) = (url.to_string(), clone_dir.clone());
        tokio::task::spawn_blocking(move || crate::git::clone_plugin_git(&u, "", &cd))
            .await
            .map_err(|e| format!("Failed to clone marketplace repository: {e}"))?
            .map_err(|e| format!("Failed to clone marketplace repository: {e}"))?;

        let index_path = clone_dir
            .join(branding::PLUGIN_MANIFEST_DIR)
            .join("marketplace.json");
        let raw = tokio::fs::read_to_string(&index_path)
            .await
            .map_err(|_| format!("Marketplace file not found at {}", index_path.display()))?;
        let index: MarketplaceIndex = serde_json::from_str(&raw)
            .map_err(|e| format!("Invalid marketplace schema: {e}"))?;
        Ok((index, clone_dir))
    }

    /// Resolve a catalog entry's plugin directory WITHIN the marketplace clone,
    /// with a path-safety guard: an attacker-controlled `path`/`pluginRoot`
    /// (e.g. `../../x`) that would escape the clone is rejected.
    pub fn plugin_dir_in_clone(
        clone_dir: &Path,
        entry: &MarketplacePluginEntry,
    ) -> Result<PathBuf, MarketplaceError> {
        if entry.external {
            return Err(format!(
                "Plugin '{}' is hosted outside the marketplace repo (external sources not yet supported)",
                entry.name
            ));
        }
        let rel = entry.path.as_deref().filter(|s| !s.is_empty()).unwrap_or(".");
        // A catalog must point only inside its own repo — reject traversal / root.
        if Path::new(rel)
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::RootDir | Component::Prefix(_)))
        {
            return Err(format!(
                "Marketplace name '{}' resolves to a path outside the cache directory",
                entry.name
            ));
        }
        Ok(clone_dir.join(rel))
    }
}
