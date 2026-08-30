//! Path to the durable record of installed plugins
//! (`<plugins>/installed_plugins.json`).
//!
//! The V2 shape (`{ version: 2, plugins: { "<plugin>@<marketplace>":
//! [{scope, installPath, version, installedAt, lastUpdated}] } }`) is written
//! ONLY by `apps/cli/src/commands/plugin_install.rs::write_installed` — the
//! production install path. [`crate::discovery`]'s reader also tolerates a
//! legacy `{ plugins: { <marketplace>: { <plugin>: {version, added} } } }`
//! shape for backward compatibility with any file already on disk, but
//! nothing in this crate writes that shape any more: an earlier revision of
//! this module carried a writer for it (`record`/`now_ms`, keyed only by
//! `PluginManager::install`'s dead git/marketplace/`.mcpb` network arms —
//! see that module's doc comment and spec §25d), which was deleted because
//! it was reachable only from tests and its shape was INCOMPATIBLE with the
//! real file's — it would have corrupted `installed_plugins.json` had it
//! ever run against a production install.

use std::path::{Path, PathBuf};

/// Path to `installed_plugins.json` under the plugins root.
#[must_use]
pub fn path(install_dir: &Path) -> PathBuf {
    install_dir.join("installed_plugins.json")
}
