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

pub mod context;
pub mod global_config;
pub mod settings_update;

#[cfg(test)]
mod test_support;
