//! `~/.claude.json` `GlobalConfig` substrate — the first in the Rust port
//! (`tools/meta/src/config.rs:17` records "no substrate" prior to this).
//!
//! Ports the path/read/save mechanics of `utils/config.ts` +
//! `utils/env.ts getGlobalClaudeFile` + `utils/envUtils.ts
//! getClaudeConfigHomeDir`, operating on a raw [`serde_json::Map`] so unknown
//! keys (the real file carries dozens: `numStartups`, `oauthAccount`, …) are
//! NEVER dropped. `serde_json`'s workspace `preserve_order` feature keeps key
//! order stable across round-trips.
//!
//! Documented simplifications vs TS (`config.ts:797-864`):
//! - No `proper-lockfile` cross-process lock and no in-memory mtime cache —
//!   migrations run once at startup before any concurrent writer exists in
//!   this process. The GH #3117 auth-loss fallback guard is therefore N/A:
//!   we never write defaults over a failed read (a broken file aborts the
//!   write instead).
//! - TS NFC-normalizes the config-home path; macOS paths are already NFC, so
//!   this port uses the path as-is.

use std::path::PathBuf;

use serde_json::{Map, Value};

/// A raw JSON object — the in-memory shape of `~/.claude.json`.
pub type JsonMap = Map<String, Value>;

/// `getClaudeConfigHomeDir` (`envUtils.ts:7-14`): `$CLAUDE_CONFIG_DIR` if
/// set, else `$HOME/.claude`. `None` when neither env var exists.
#[must_use]
pub fn claude_config_home() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        return Some(PathBuf::from(dir));
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".claude"))
}

/// `getGlobalClaudeFile` (`env.ts:14-26`): legacy `<config-home>/.config.json`
/// when it exists, else `($CLAUDE_CONFIG_DIR || $HOME)/.claude.json`.
///
/// The TS oauth filename suffix (`fileSuffixForOauthConfig()` →
/// `-custom-oauth`/`-local-oauth`/`-staging-oauth`) only applies under custom
/// OAuth env vars this port does not model (`anthropic-oauth` has no
/// `getOauthConfigType` substrate) — the default build resolves it to `""`,
/// so `.claude.json` is hardcoded here.
#[must_use]
pub fn global_config_path() -> Option<PathBuf> {
    let home = claude_config_home()?;
    let legacy = home.join(".config.json");
    if legacy.exists() {
        return Some(legacy);
    }
    let base = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))?;
    Some(base.join(".claude.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::env_lock;

    #[test]
    fn config_home_prefers_claude_config_dir() {
        let _g = env_lock();
        std::env::set_var("CLAUDE_CONFIG_DIR", "/tmp/cc-test-home");
        assert_eq!(
            claude_config_home(),
            Some(std::path::PathBuf::from("/tmp/cc-test-home"))
        );
        std::env::remove_var("CLAUDE_CONFIG_DIR");
    }

    #[test]
    fn config_home_falls_back_to_home_dot_claude() {
        let _g = env_lock();
        std::env::remove_var("CLAUDE_CONFIG_DIR");
        std::env::set_var("HOME", "/tmp/cc-test-h2");
        assert_eq!(
            claude_config_home(),
            Some(std::path::PathBuf::from("/tmp/cc-test-h2/.claude"))
        );
    }

    #[test]
    fn global_path_prefers_legacy_config_json_when_present() {
        let _g = env_lock();
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("CLAUDE_CONFIG_DIR", tmp.path());
        std::fs::write(tmp.path().join(".config.json"), "{}").unwrap();
        assert_eq!(global_config_path(), Some(tmp.path().join(".config.json")));
        std::env::remove_var("CLAUDE_CONFIG_DIR");
    }

    #[test]
    fn global_path_is_claude_json_under_config_dir_else_home() {
        let _g = env_lock();
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("CLAUDE_CONFIG_DIR", tmp.path());
        assert_eq!(global_config_path(), Some(tmp.path().join(".claude.json")));
        std::env::remove_var("CLAUDE_CONFIG_DIR");
        std::env::set_var("HOME", "/tmp/cc-test-h3");
        assert_eq!(
            global_config_path(),
            Some(std::path::PathBuf::from("/tmp/cc-test-h3/.claude.json"))
        );
    }
}
