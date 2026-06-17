//! Allowlist-driven discovery against the REAL claude-code on-disk layout
//! (GAP E verification fix #1).
//!
//! claude-code never flat-walks `plugins/*/` for manifests. Its cache-only
//! loader (`loadAllPluginsCacheOnly` → `loadPluginsFromMarketplaces`,
//! `pluginLoader.ts:1888`) is driven by `settings.enabledPlugins` — a map of
//! `plugin@marketplace` → enabled — and resolves each enabled entry through
//! `getVersionedCachePath` (`pluginLoader.ts:139`) to the versioned cache path
//!
//!     <plugins>/cache/{marketplace}/{plugin}/{version}/
//!
//! The plugin manifest lives at that versioned dir. `discover_enabled_plugins`
//! ports that resolution: it reads only the allowlisted entries and resolves
//! them to their versioned cache directory, so against a real plugins dir it
//! discovers exactly the enabled plugins (not zero, and not "every dir").

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// Materialize a versioned cache entry exactly as claude-code lays it out:
/// `<root>/cache/{marketplace}/{plugin}/{version}/.claude-plugin/plugin.json`.
fn write_cached_plugin(root: &Path, marketplace: &str, plugin: &str, version: &str) {
    let versioned = root
        .join("cache")
        .join(marketplace)
        .join(plugin)
        .join(version);
    fs::create_dir_all(versioned.join(".claude-plugin")).unwrap();
    fs::write(
        versioned.join(".claude-plugin").join("plugin.json"),
        format!(r#"{{"name":"{plugin}","version":"{version}"}}"#),
    )
    .unwrap();
    fs::create_dir_all(versioned.join("commands")).unwrap();
    fs::write(
        versioned.join("commands").join("hi.md"),
        "---\ndescription: hi\n---\nbody\n",
    )
    .unwrap();
}

#[tokio::test]
async fn resolves_enabled_entries_to_versioned_cache_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // Two cached plugins, one enabled, one disabled in the allowlist.
    write_cached_plugin(root, "acme", "weather", "1.0.0");
    write_cached_plugin(root, "acme", "calc", "2.3.1");

    // Real claude-code top-level junk that must NOT be mistaken for a plugin.
    fs::create_dir_all(root.join("npm-cache")).unwrap();
    fs::write(root.join("installed_plugins.json"), "{}").unwrap();

    let mut enabled: BTreeMap<String, bool> = BTreeMap::new();
    enabled.insert("weather@acme".to_string(), true);
    enabled.insert("calc@acme".to_string(), false); // disabled → skipped

    let discovered = plugin::discover_enabled_plugins(root, &enabled).await;

    assert_eq!(
        discovered.len(),
        1,
        "only the enabled allowlisted plugin is discovered"
    );
    let (_, manifest, dir) = &discovered[0];
    assert_eq!(manifest.name, "weather");
    assert_eq!(
        dir,
        &root.join("cache").join("acme").join("weather").join("1.0.0"),
        "resolved to the versioned cache dir"
    );
    assert_eq!(manifest.components.commands.len(), 1);
}

#[tokio::test]
async fn flat_walk_of_a_real_plugins_dir_finds_nothing() {
    // The real ~/.claude/plugins holds cache/, npm-cache/, installed_plugins.json
    // — none with a direct .claude-plugin/plugin.json child. The legacy flat
    // walk must therefore find ZERO (documents the layout mismatch the
    // allowlist path fixes).
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_cached_plugin(root, "acme", "weather", "1.0.0");
    fs::create_dir_all(root.join("npm-cache")).unwrap();

    let flat = plugin::discover_installed_plugins(root).await;
    assert!(
        flat.is_empty(),
        "flat walk finds nothing against the real cache/ layout"
    );
}

#[tokio::test]
async fn missing_version_dir_for_enabled_entry_is_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Enabled but never installed → no cache dir.
    let mut enabled: BTreeMap<String, bool> = BTreeMap::new();
    enabled.insert("ghost@acme".to_string(), true);

    let discovered = plugin::discover_enabled_plugins(root, &enabled).await;
    assert!(discovered.is_empty(), "uninstalled enabled entry is skipped");
}
