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

<a id="plugin-directory"></a>

## Prepare a local plugin directory

For PluginSource::LocalPath, provide an already materialized directory containing .lingxi-plugin/plugin.json. The discovery path can find optional commands/, agents/, skills/, output-styles/ and hooks/hooks.json. Keep those files available while the runtime uses the discovered components.

1. Prepare the directory with a manifest and the component files it declares. Validate the content before activation.
2. Pass its path to the host-assembled PluginManager. Record the returned PluginId, then inspect plugin_state for the actual result.
3. Check the contributed registry entries rather than assuming every optional component became active.
4. When removing the plugin, call disable on the same manager and retry any retained DisablingFailed teardown.

### Local plugin layout

```text
report-helper/
  .lingxi-plugin/plugin.json
  commands/
  agents/
  skills/
  hooks/hooks.json
```

Verification method: API excerpt

<a id="plugin-install-paths"></a>

## Select the correct install path

| Source | PluginManager::install | Host action |
| --- | --- | --- |
| LocalPath | Discovers and enables the prepared directory in place. | Provide a readable manifest and retain the directory lifecycle. |
| Git / Marketplace / OfficialMarketplace | Returns a typed unsupported/fetch-path error. | Use the production installation/materialization pipeline before activation. |
| Mcpb | Does not unpack the bundle through this method. | Use the supported host install pipeline for the bundle. |
| BuiltIn | Is not installed through this method. | Use the composition’s compiled builtin registration. |

A PluginSource variant describes an input source. Its existence does not promise that this particular manager method implements network fetching, authentication or archive materialization.

<a id="plugin-troubleshooting"></a>

## Diagnose activation and removal

| Observation | Interpretation | Recovery |
| --- | --- | --- |
| No manifest found | A flat LocalPath discovery did not find the expected plugin directory. | Check the exact directory and .lingxi-plugin/plugin.json; do not pass the top-level versioned cache as a plugin. |
| Blocked | Policy rejected the source before activation. | Inspect trust and managed policy; do not register the components manually to bypass it. |
| Failed / Validation / Loader | Manifest, configuration or loading failed. | Correct the reported cause before enabling the candidate again. |
| DisablingFailed remains loaded | Some owned resources still need teardown. | Retain the owner and retry disable; report success only after its components are removed. |

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/plugin/src/manager.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/plugin/src/manager.rs) — Committed source (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/plugin/src/lifecycle.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/plugin/src/lifecycle.rs) — Committed source (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/plugin/src/manifest.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/plugin/src/manifest.rs) — Committed source (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/plugin/src/discovery.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/plugin/src/discovery.rs) — Committed source (a39da75f0985d61d90fffa255f7b4d90901b0e27)

- harness-runtime: HEAD a39da75f0985d61d90fffa255f7b4d90901b0e27; Referenced input fingerprint sha256:92c2108dc63215bc69ea9c11ea081795da883fdcb1c9c28b060aa9221a1b95a2; Product pin a39da75f0985d61d90fffa255f7b4d90901b0e27

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
