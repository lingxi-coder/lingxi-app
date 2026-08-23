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
    /// Optional marketplace-wide metadata bag.
    #[serde(default)]
    pub metadata: Option<MarketplaceIndexMetadata>,
    /// Marketplaces that dependency declarations may explicitly cross into.
    #[serde(rename = "allowCrossMarketplaceDependenciesOn", default)]
    pub allow_cross_marketplace_dependencies_on: Vec<String>,
}

/// Optional marketplace-wide metadata accepted by marketplace catalogs.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct MarketplaceIndexMetadata {
    /// Plugin root prepended to safe bare-string entry sources.
    #[serde(default, alias = "pluginRoot")]
    pub plugin_root: Option<String>,
}

/// Typed source of one plugin catalog entry.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum MarketplacePluginSource {
    /// A path relative to the marketplace root.
    Relative(String),
    /// A structured external or directory source.
    Structured(MarketplaceExternalSource),
}

/// Structured source variants accepted by marketplace catalogs.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "source", rename_all = "lowercase")]
pub enum MarketplaceExternalSource {
    /// GitHub repository source.
    Github {
        repo: String,
        #[serde(rename = "ref", default)]
        git_ref: Option<String>,
        #[serde(default)]
        path: Option<String>,
    },
    /// Arbitrary git repository source.
    Git {
        url: String,
        #[serde(rename = "ref", default)]
        git_ref: Option<String>,
        #[serde(default)]
        path: Option<String>,
    },
    /// HTTPS archive source.
    Url { url: String },
    /// npm package source.
    Npm { package: String },
    /// File source.
    File { path: String },
    /// Directory source.
    Directory { path: String },
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
    /// Canonical source; a relative string remains inside the marketplace.
    #[serde(default)]
    pub source: Option<MarketplacePluginSource>,
    /// A plugin hosted OUTSIDE the marketplace repo (git/url sub-source). Not
    /// yet materialized (follow-up); flagged so the arm can error clearly.
    #[serde(default)]
    pub external: bool,
    /// Dependencies contributed by the marketplace entry.
    #[serde(default)]
    pub dependencies: Option<serde_json::Value>,
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
        self.resolve_index_via_git_ref(url, name, None).await
    }

    /// Clone a marketplace at an optional branch or tag.
    pub async fn resolve_index_via_git_ref(
        &self,
        url: &str,
        name: &str,
        git_ref: Option<&str>,
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
        let (u, r, cd) = (
            url.to_string(),
            git_ref.unwrap_or_default().to_string(),
            clone_dir.clone(),
        );
        tokio::task::spawn_blocking(move || crate::git::clone_plugin_git(&u, &r, &cd))
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
            .map(Self::normalize_index_plugin_root)
            .map_err(|e| format!("Invalid marketplace schema: {e}"))?;
        Ok((index, clone_dir))
    }

    fn normalize_index_plugin_root(mut index: MarketplaceIndex) -> MarketplaceIndex {
        let Some(plugin_root) = index
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.plugin_root.as_deref())
            .map(str::trim)
            .filter(|root| !root.is_empty())
        else {
            return index;
        };

        for entry in &mut index.plugins {
            let Some(MarketplacePluginSource::Relative(source)) = entry.source.as_mut() else {
                continue;
            };
            if !is_safe_bare_relative_source(source) {
                continue;
            }
            *source = normalized_source_with_root(plugin_root, source);
        }
        index
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
        let source_path = match entry.source.as_ref() {
            Some(MarketplacePluginSource::Relative(path)) => Some(path.as_str()),
            Some(MarketplacePluginSource::Structured(
                MarketplaceExternalSource::Directory { path }
                | MarketplaceExternalSource::File { path },
            )) => Some(path.as_str()),
            Some(MarketplacePluginSource::Structured(_)) => {
                return Err(format!(
                    "Plugin '{}' is hosted outside the marketplace repo",
                    entry.name
                ));
            }
            None => None,
        };
        let rel = PathBuf::from(
            source_path
                .or(entry.path.as_deref())
                .filter(|s| !s.is_empty())
                .unwrap_or("."),
        );
        // A catalog must point only inside its own repo — reject traversal / root.
        if rel.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            return Err(format!(
                "Marketplace name '{}' resolves to a path outside the cache directory",
                entry.name
            ));
        }
        Ok(clone_dir.join(rel))
    }
}

fn is_safe_bare_relative_source(source: &str) -> bool {
    if source.is_empty() || source.starts_with('.') {
        return false;
    }
    !Path::new(source).components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    })
}

fn normalized_source_with_root(plugin_root: &str, source: &str) -> String {
    let mut rel = PathBuf::from(".");
    if plugin_root != "." {
        rel.push(plugin_root);
    }
    rel.push(source);
    rel.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_metadata_plugin_root_rewrites_safe_bare_source() {
        let index: MarketplaceIndex = serde_json::from_value(serde_json::json!({
            "name": "demo-market",
            "metadata": { "pluginRoot": "plugins/demo" },
            "plugins": [{ "name": "demo", "source": "bundle" }]
        }))
        .map(MarketplaceManager::normalize_index_plugin_root)
        .expect("parse index");

        let dir =
            MarketplaceManager::plugin_dir_in_clone(Path::new("/tmp/clone"), &index.plugins[0])
                .expect("resolve inside clone");
        assert_eq!(dir, Path::new("/tmp/clone/./plugins/demo/bundle"));
    }

    #[test]
    fn top_level_plugin_root_stays_compatible() {
        let entry: MarketplacePluginEntry = serde_json::from_value(serde_json::json!({
            "name": "demo",
            "pluginRoot": "plugins/demo"
        }))
        .expect("parse entry");

        let dir = MarketplaceManager::plugin_dir_in_clone(Path::new("/tmp/clone"), &entry)
            .expect("resolve legacy plugin root");
        assert_eq!(dir, Path::new("/tmp/clone/plugins/demo"));
    }

    #[test]
    fn index_metadata_plugin_root_does_not_rewrite_explicit_dot_path() {
        let index: MarketplaceIndex = serde_json::from_value(serde_json::json!({
            "name": "demo-market",
            "metadata": { "pluginRoot": "plugins/demo" },
            "plugins": [{ "name": "demo", "source": "./bundle" }]
        }))
        .map(MarketplaceManager::normalize_index_plugin_root)
        .expect("parse index");

        match index.plugins[0].source.as_ref() {
            Some(MarketplacePluginSource::Relative(source)) => assert_eq!(source, "./bundle"),
            other => panic!("expected relative source, got {other:?}"),
        }
    }

    #[test]
    fn index_metadata_plugin_root_does_not_mask_escape() {
        let index: MarketplaceIndex = serde_json::from_value(serde_json::json!({
            "name": "demo-market",
            "metadata": { "pluginRoot": "plugins/demo" },
            "plugins": [{ "name": "demo", "source": "../secret" }]
        }))
        .map(MarketplaceManager::normalize_index_plugin_root)
        .expect("parse index");

        let err =
            MarketplaceManager::plugin_dir_in_clone(Path::new("/tmp/clone"), &index.plugins[0])
                .expect_err("unsafe source must still be rejected");
        assert!(err.contains("outside the cache directory"), "got: {err}");
    }
}
