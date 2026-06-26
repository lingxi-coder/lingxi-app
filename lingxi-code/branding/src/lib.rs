//! Single source of truth for the product namespace: on-disk directory names,
//! well-known filenames, the config-dir env override name, the env-var prefix,
//! and the brand name. Every crate that needs one of these values imports it
//! from here so the namespace is defined in exactly one place.
//!
//! These hold LingXi's namespace values. (They were introduced holding the
//! Claude values so routing the scattered literals through this crate was a
//! pure refactor; this commit flips them to LingXi, which is why the parity
//! fixtures/tests were updated together.)
//!
//! The Anthropic *protocol* layer (model IDs, `anthropic` host/provider,
//! `tengu_*`, beta headers, `claude-cli` User-Agent, OAuth, `ANTHROPIC_*` env)
//! is deliberately NOT defined here — those must stay Claude/Anthropic for the
//! backend to work and live in their own modules.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// User config directory name under `$HOME` (e.g. `~/.lingxi`). Also the
/// per-project config dir name (`<repo>/.lingxi/`).
pub const DOT_DIR: &str = ".lingxi";

/// Global config file, a sibling of [`DOT_DIR`] in `$HOME` (e.g. `~/.lingxi.json`).
pub const GLOBAL_CONFIG_FILE: &str = ".lingxi.json";

/// Legacy global-config filename checked *inside* the config-home before
/// [`GLOBAL_CONFIG_FILE`] (e.g. `<config-home>/.config.json`). The filename
/// itself is not brand-specific; named here so the resolver has one source.
pub const LEGACY_GLOBAL_CONFIG_FILE: &str = ".config.json";

/// Environment variable that overrides the config-home directory.
pub const CONFIG_DIR_ENV: &str = "LINGXI_CONFIG_DIR";

/// Project memory filename (case-sensitive).
pub const MEMORY_FILE: &str = "LINGXI.md";

/// Local-override memory filename.
pub const MEMORY_LOCAL_FILE: &str = "LINGXI.local.md";

/// Manifest directory name inside a plugin / marketplace package.
pub const PLUGIN_MANIFEST_DIR: &str = ".lingxi-plugin";

/// Human-facing product name (banners, system-prompt identity, help text).
pub const PRODUCT_NAME: &str = "LingXi";

/// Prefix for the product's own (non-protocol) environment variables, e.g.
/// `LINGXI_ENABLE_TASKS`. Protocol env vars (`ANTHROPIC_*`, the kept
/// `CLAUDE_CODE_*` SDK contract vars) are excluded from this prefix by design.
pub const ENV_PREFIX: &str = "LINGXI_";

/// Managed/enterprise policy directory per platform (admin-provisioned).
/// macOS Application Support location.
pub const MANAGED_DIR_MACOS: &str = "/Library/Application Support/LingXi";
/// Windows Program Files location.
pub const MANAGED_DIR_WINDOWS: &str = r"C:\Program Files\LingXi";
/// Other (Linux/BSD) location.
pub const MANAGED_DIR_UNIX: &str = "/etc/lingxi";

/// Resolve the user config-home: `$LINGXI_CONFIG_DIR` when the env value is
/// supplied (honored verbatim, including an empty value — matching the upstream
/// `??` semantics), else `home.join(DOT_DIR)`.
///
/// This is the pure core: the env value is injected so callers stay testable
/// without mutating process env. Callers that historically treated an *empty*
/// value as unset (the `||`-shaped resolvers, e.g. `migrations`) should filter
/// the env value to `None` before calling and pass their own home, preserving
/// that deliberate divergence.
#[must_use]
pub fn config_home(home: &Path, config_dir_env: Option<OsString>) -> PathBuf {
    match config_dir_env {
        Some(dir) => PathBuf::from(dir),
        None => home.join(DOT_DIR),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn config_home_defaults_to_home_join_dot_dir() {
        let got = config_home(Path::new("/home/u"), None);
        assert_eq!(got, Path::new("/home/u").join(DOT_DIR));
    }

    #[test]
    fn config_home_honors_env_verbatim_including_empty() {
        // `??` semantics: a SET value wins verbatim, even when empty.
        let got = config_home(Path::new("/home/u"), Some("/custom".into()));
        assert_eq!(got, Path::new("/custom"));
        let empty = config_home(Path::new("/home/u"), Some(OsString::from("")));
        assert_eq!(empty, Path::new(""));
    }

    #[test]
    fn namespace_values_are_lingxi() {
        assert_eq!(DOT_DIR, ".lingxi");
        assert_eq!(GLOBAL_CONFIG_FILE, ".lingxi.json");
        assert_eq!(CONFIG_DIR_ENV, "LINGXI_CONFIG_DIR");
        assert_eq!(MEMORY_FILE, "LINGXI.md");
        assert_eq!(MEMORY_LOCAL_FILE, "LINGXI.local.md");
        assert_eq!(PLUGIN_MANIFEST_DIR, ".lingxi-plugin");
        assert_eq!(PRODUCT_NAME, "LingXi");
        assert_eq!(ENV_PREFIX, "LINGXI_");
        // No Claude namespace leaks in our own values.
        for v in [DOT_DIR, GLOBAL_CONFIG_FILE, CONFIG_DIR_ENV, MEMORY_FILE,
                  MEMORY_LOCAL_FILE, PLUGIN_MANIFEST_DIR, ENV_PREFIX] {
            assert!(!v.to_lowercase().contains("claude"), "claude leak: {v}");
        }
    }
}
