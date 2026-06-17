//! Bootstrap discovery of installed plugins from disk (GAP E).
//!
//! Mirrors claude-code's cache-only loader (`pluginLoader.ts:1348`
//! `createPluginFromPath` + `1618` standard `hooks/hooks.json`): walk the
//! plugins directory, read each `<plugin>/.claude-plugin/plugin.json`, and
//! auto-detect the `commands/`, `agents/`, `skills/`, `output-styles/`
//! component directories plus the standard `hooks/hooks.json`.

use std::fs;
use std::path::Path;

/// Write a self-contained fixture plugin under `root/<name>`.
fn write_fixture_plugin(root: &Path, name: &str) {
    let plugin_dir = root.join(name);
    fs::create_dir_all(plugin_dir.join(".claude-plugin")).unwrap();
    fs::write(
        plugin_dir.join(".claude-plugin").join("plugin.json"),
        r#"{"name":"myplugin","version":"1.2.3","description":"a fixture plugin","author":{"name":"Ada"},"homepage":"https://example.com"}"#,
    )
    .unwrap();

    fs::create_dir_all(plugin_dir.join("commands")).unwrap();
    fs::write(
        plugin_dir.join("commands").join("hello.md"),
        "---\ndescription: say hi\n---\nHello from the plugin.\n",
    )
    .unwrap();

    fs::create_dir_all(plugin_dir.join("agents")).unwrap();
    fs::write(
        plugin_dir.join("agents").join("helper.md"),
        "---\nname: helper\ndescription: helps\n---\nYou help.\n",
    )
    .unwrap();

    fs::create_dir_all(plugin_dir.join("hooks")).unwrap();
    fs::write(
        plugin_dir.join("hooks").join("hooks.json"),
        r#"{"description":"fixture hooks","hooks":{"PreToolUse":[{"matcher":"Write","hooks":[{"type":"command","command":"echo hi"}]}]}}"#,
    )
    .unwrap();
}

#[tokio::test]
async fn discovers_a_single_installed_plugin_with_all_components() {
    let tmp = tempfile::tempdir().unwrap();
    write_fixture_plugin(tmp.path(), "myplugin");

    let discovered = plugin::discover_installed_plugins(tmp.path()).await;

    assert_eq!(discovered.len(), 1, "exactly one plugin discovered");
    let (id, manifest, dir) = &discovered[0];

    // Manifest metadata parsed from plugin.json.
    assert_eq!(manifest.name, "myplugin");
    assert_eq!(manifest.version, "1.2.3");
    assert_eq!(manifest.description, "a fixture plugin");
    assert_eq!(manifest.author.as_deref(), Some("Ada"));
    assert_eq!(manifest.homepage.as_deref(), Some("https://example.com"));

    // The freshly-minted id is stamped onto the manifest too.
    assert_eq!(manifest.id, *id);

    // Auto-detected component directories (createPluginFromPath Step 3).
    assert_eq!(
        manifest.components.commands.len(),
        1,
        "commands/hello.md auto-detected"
    );
    assert_eq!(
        manifest.components.agents.len(),
        1,
        "agents/helper.md auto-detected"
    );

    // Standard hooks/hooks.json parsed into HookDefinition(s).
    assert_eq!(
        manifest.components.hooks.len(),
        1,
        "hooks/hooks.json PreToolUse entry parsed"
    );

    // The install dir points at the plugin root.
    assert_eq!(dir, &tmp.path().join("myplugin"));
}

#[tokio::test]
async fn missing_plugins_dir_yields_no_plugins() {
    let tmp = tempfile::tempdir().unwrap();
    let absent = tmp.path().join("does-not-exist");
    let discovered = plugin::discover_installed_plugins(&absent).await;
    assert!(discovered.is_empty());
}

#[tokio::test]
async fn a_dir_without_a_manifest_is_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    // A stray directory with no `.claude-plugin/plugin.json`.
    fs::create_dir_all(tmp.path().join("not-a-plugin").join("commands")).unwrap();
    let discovered = plugin::discover_installed_plugins(tmp.path()).await;
    assert!(
        discovered.is_empty(),
        "directories without a manifest are not plugins"
    );
}
