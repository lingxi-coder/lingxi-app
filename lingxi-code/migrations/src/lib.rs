//! Startup config migrations — port of claude-code `runMigrations()`
//! (`main.tsx:323-353`, `CURRENT_MIGRATION_VERSION = 11`) plus the
//! `~/.claude.json` `GlobalConfig` substrate it requires (`utils/config.ts`).
//!
//! Desktop-only: wired in `apps/cli` pre-REPL; NEVER part of the
//! engine-mobile dependency tree.
//!
//! Excluded from the port (with reasons): `migrateFennecToOpus` (dead code in
//! the external build — `if ("external" === 'ant')`), and
//! `resetAutoModeOptInForDefaultOffer` (gated on the ant-only
//! `TRANSCRIPT_CLASSIFIER` feature; the classifier is correctly stubbed in
//! this port).

#![forbid(unsafe_code)]

pub mod changelog;
pub mod context;
pub mod global_config;
pub mod migrate_auto_updates;
pub mod migrate_bypass_permissions;
pub mod migrate_legacy_opus;
pub mod migrate_mcp_servers;
pub mod migrate_opus_to_opus1m;
pub mod migrate_repl_bridge;
pub mod migrate_reset_pro_to_opus;
pub mod migrate_sonnet1m_to_sonnet45;
pub mod migrate_sonnet45_to_46;
pub mod runner;
pub mod settings_update;

pub use changelog::migrate_changelog_from_config;
pub use context::{MigrationContext, MigrationEnv};
pub use runner::{run_migrations, CURRENT_MIGRATION_VERSION};

#[cfg(test)]
mod test_support;
