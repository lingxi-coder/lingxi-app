//! Enterprise MCP policy — a byte-faithful port of claude-code 2.1.206's
//! managed-MCP gates.
//!
//! When an organization ships a managed MCP configuration, `claude mcp add`
//! runs three checks (in `TPe`/`addMcpServer`, after the reserved-name check
//! and before any scope write):
//!
//! 1. **`D1()`** — is enterprise MCP configuration *active*? If a managed
//!    `managed-mcp.json` exists and parses, the enterprise config has exclusive
//!    control and every add is refused.
//! 2. **`bPe(name, config)`** — is the server *explicitly denied* by policy?
//! 3. **`gPe(name, config)`** — is the server *allowed* by policy?
//!
//! This module ports the substrate. Stage 1 (this file) implements the managed
//! path resolution and `D1()`; the `bPe`/`gPe` allow/deny matchers land in a
//! follow-up so each step stays non-divergent (`D1` is checked first, so when
//! it fires the matchers never run).

use std::path::{Path, PathBuf};

/// Env override relocating the managed (policy) settings root — the same
/// variable the engine's `managed_settings_dir` honors, so a test (or an
/// unusual deployment) can point both at one directory. Unset in production.
pub const MANAGED_DIR_ENV: &str = "LINGXI_MANAGED_DIR";

/// The OS-specific managed settings root — claude-code's `XM()`
/// (`getManagedFilePath`), LingXi-branded (see [`branding`]). Honors
/// [`MANAGED_DIR_ENV`] first (test/relocation), else the hardcoded OS path.
#[must_use]
pub fn managed_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(MANAGED_DIR_ENV).filter(|v| !v.is_empty()) {
        return PathBuf::from(dir);
    }
    if cfg!(target_os = "macos") {
        PathBuf::from(branding::MANAGED_DIR_MACOS)
    } else if cfg!(target_os = "windows") {
        PathBuf::from(branding::MANAGED_DIR_WINDOWS)
    } else {
        PathBuf::from(branding::MANAGED_DIR_UNIX)
    }
}

/// claude-code `qFr()` — the managed MCP config path,
/// `<managed_dir>/managed-mcp.json`.
#[must_use]
pub fn managed_mcp_config_path() -> PathBuf {
    managed_dir().join("managed-mcp.json")
}

/// claude-code `D1()` — is enterprise MCP configuration active?
///
/// `D1` memoizes `$7t({filePath: qFr(), scope:"enterprise"}).config !== null`:
/// the managed MCP config file is read and parsed, and the config is non-null
/// exactly when it is a regular file within the size limit holding a valid JSON
/// object. An absent, empty, oversized, or malformed file → not active.
#[must_use]
pub fn enterprise_mcp_active() -> bool {
    enterprise_mcp_active_at(&managed_mcp_config_path())
}

/// claude-code's byte-exact `mcp add` rejection when [`enterprise_mcp_active`].
pub const ENTERPRISE_EXCLUSIVE_CONTROL_MESSAGE: &str =
    "Cannot add MCP server: enterprise MCP configuration is active and has exclusive control over MCP servers";

/// The [`enterprise_mcp_active`] core, parameterized on the config path so it is
/// testable without touching the process-global managed dir / env.
#[must_use]
pub fn enterprise_mcp_active_at(path: &Path) -> bool {
    // claude's `$7t` gates on "regular file within size limit"; here a failed
    // read (ENOENT / not-a-file / unreadable) is the same not-active signal.
    let Ok(raw) = std::fs::read_to_string(path) else {
        return false;
    };
    if raw.trim().is_empty() {
        return false;
    }
    // `config !== null` ⟺ the file parsed to a JSON object. Malformed JSON or a
    // non-object top level yields a null config (not active).
    matches!(
        serde_json::from_str::<serde_json::Value>(&raw),
        Ok(serde_json::Value::Object(_))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &tempfile::TempDir, name: &str, body: &str) -> PathBuf {
        let p = dir.path().join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn absent_file_is_not_active() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!enterprise_mcp_active_at(&dir.path().join("managed-mcp.json")));
    }

    #[test]
    fn valid_json_object_is_active() {
        let dir = tempfile::tempdir().unwrap();
        // Any valid JSON object → config non-null → active (matches `$7t`,
        // which does not require an `mcpServers` key at this gate).
        let p = write(&dir, "managed-mcp.json", r#"{"mcpServers":{"corp":{"type":"stdio","command":"c"}}}"#);
        assert!(enterprise_mcp_active_at(&p));
        let p2 = write(&dir, "empty-obj.json", "{}");
        assert!(enterprise_mcp_active_at(&p2));
    }

    #[test]
    fn empty_or_malformed_or_non_object_is_not_active() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!enterprise_mcp_active_at(&write(&dir, "empty.json", "")));
        assert!(!enterprise_mcp_active_at(&write(&dir, "ws.json", "   \n ")));
        assert!(!enterprise_mcp_active_at(&write(&dir, "bad.json", "{not json")));
        // A valid JSON value that is not an object → null config.
        assert!(!enterprise_mcp_active_at(&write(&dir, "arr.json", "[1,2,3]")));
        assert!(!enterprise_mcp_active_at(&write(&dir, "str.json", "\"hi\"")));
    }

    #[test]
    fn message_is_byte_exact() {
        assert_eq!(
            ENTERPRISE_EXCLUSIVE_CONTROL_MESSAGE,
            "Cannot add MCP server: enterprise MCP configuration is active and has exclusive control over MCP servers"
        );
    }

    #[test]
    fn managed_mcp_path_is_managed_dir_plus_filename() {
        // Isolate from any real managed dir via the env override.
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: single-threaded test; restored immediately after.
        let prev = std::env::var_os(MANAGED_DIR_ENV);
        std::env::set_var(MANAGED_DIR_ENV, dir.path());
        assert_eq!(managed_mcp_config_path(), dir.path().join("managed-mcp.json"));
        match prev {
            Some(v) => std::env::set_var(MANAGED_DIR_ENV, v),
            None => std::env::remove_var(MANAGED_DIR_ENV),
        }
    }
}
