# Plugins

Load plugin components, inspect their lifecycle, and handle unload failures.

<a id="components"></a>

## Plugin components

The plugin crate exports PluginManifest, PluginComponents, PluginSource, PluginState, PluginManager, strict policy, trust, and discovery helpers. Declared components include commands, agents, skills, hooks, output styles, MCP servers, LSP servers, and workflows. The manager materializes these into the host’s registries.

The host supplies registry, filesystem, and related dependencies when constructing PluginManager. This is a companion crate interface; Harness does not expose a mutable plugin registry.

<a id="local-install"></a>

## Install a prepared local plugin

PluginManager::install currently supports PluginSource::LocalPath. It discovers the existing directory in place, reads its manifest and components, enables the plugin, and returns its PluginId. The manager’s Git, marketplace, MCP bundle, and builtin install branches return typed errors. Network materialization belongs to the host’s production install pipeline.

### Rust · host-assembled PluginManager required

```rust
use std::path::PathBuf;
use plugin::{PluginManager, PluginManagerError, PluginSource};

async fn install_prepared(
    manager: &PluginManager,
    directory: PathBuf,
) -> Result<(), PluginManagerError> {
    let id = manager.install(
        PluginSource::LocalPath { path: directory },
    ).await?;
    let _state = manager.plugin_state(&id).await;
    Ok(())
}
```

### PluginManager::install

```text
pub async fn install(&self, source: PluginSource) -> Result<PluginId, PluginManagerError>
```

Discover and enable a local plugin directory using the existing host registries.

### PluginManager::enable

```text
pub async fn enable(&self, id: &PluginId, manifest: PluginManifest, install_dir: PathBuf) -> Result<(), PluginManagerError>
```

Load an already materialized manifest and install directory into the registries.

<a id="lifecycle"></a>

## Track plugin lifecycle

- Declared → Fetching → Fetched → Loaded describes preparation and activation.
- Disabled means contributed components have been removed. Failed and Blocked retain failure or policy context.
- DisablingFailed preserves a partial unload failure so a later disable pass can retry the remaining teardown.

### PluginManager::plugin_state

```text
pub async fn plugin_state(&self, id: &PluginId) -> Option<PluginState>
```

Read a snapshot of one plugin’s lifecycle state.

### PluginManager::loaded_plugin_ids

```text
pub async fn loaded_plugin_ids(&self) -> Vec<PluginId>
```

Include Loaded and DisablingFailed plugins so unload failures remain visible to retry passes.

### PluginManager::disable

```text
pub async fn disable(&self, id: &PluginId) -> Result<(), PluginManagerError>
```

Unload contributed components and transition to Disabled, or retain DisablingFailed on teardown error.

<a id="policy"></a>

## Apply host policy

Plugin trust, managed marketplace restrictions, strict plugin-only policy, component validation, and user-config resolution participate in loading. Use the existing manager and discovery path so plugin components pass through those controls before they mutate registries.

A parsed manifest field is not evidence that its optional fetch or runtime behavior is implemented. The linked source identifies the enabled component paths and explicit unsupported branches.

<a id="teardown-recovery"></a>

## Refresh and recover registry ownership

Enablement contributes components through the production manager; disabling must remove those exact contributions and stop owned workers. A DisablingFailed plugin stays visible to loaded_plugin_ids for retry, but is not authority for replaying a live Mod. Keep the failure and retry teardown before claiming the plugin is unloaded.

A refreshed MCP or LSP catalog is a live registry update, not a second builtin copy. Plugin-supplied workflows enter the same workflow registry used by the runtime launcher. Marketplace parsing, trust policy and successful runtime activation are separate checkpoints.

### Plugin unit and LSP registration checks

```bash
cargo test --locked -p plugin --lib
cargo test --locked -p lsp --test plugin_only_registration_test
```

Verification method: Environment validation

Existing tests exercise component cleanup, failure retention and verified-plugin-only LSP registration. Installing an actual third-party package additionally requires host trust, transport and process cleanup acceptance.

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/plugin/src/manager.rs](https://github.com/lingxi-coder/harness-runtime/blob/e3e34efba151a8219e2b05406c3d229b5cc6314f/crates/plugin/src/manager.rs) — Committed source (e3e34efba151a8219e2b05406c3d229b5cc6314f)
- [harness-runtime/crates/plugin/src/lifecycle.rs](https://github.com/lingxi-coder/harness-runtime/blob/e3e34efba151a8219e2b05406c3d229b5cc6314f/crates/plugin/src/lifecycle.rs) — Committed source (e3e34efba151a8219e2b05406c3d229b5cc6314f)
- [harness-runtime/crates/plugin/src/manifest.rs](https://github.com/lingxi-coder/harness-runtime/blob/e3e34efba151a8219e2b05406c3d229b5cc6314f/crates/plugin/src/manifest.rs) — Committed source (e3e34efba151a8219e2b05406c3d229b5cc6314f)
- [harness-runtime/crates/plugin/src/discovery.rs](https://github.com/lingxi-coder/harness-runtime/blob/e3e34efba151a8219e2b05406c3d229b5cc6314f/crates/plugin/src/discovery.rs) — Committed source (e3e34efba151a8219e2b05406c3d229b5cc6314f)

- harness-runtime: HEAD e3e34efba151a8219e2b05406c3d229b5cc6314f; Referenced input fingerprint sha256:2d439c83e9c12bcb5aacfcf0a5da70a9d8beba68e9eba861d817bcf67c9bb51b; Product pin e3e34efba151a8219e2b05406c3d229b5cc6314f

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
