//! `.mcp.json` parser (M6-07).
//!
//! Reads the claude-code-compatible shape:
//! ```json
//! {
//!   "mcpServers": {
//!     "memory":     { "command": "mcp-memory",     "args": [], "env": {} },
//!     "filesystem": { "command": "mcp-filesystem", "args": ["/tmp"], "env": {} }
//!   }
//! }
//! ```
//! Each entry is projected into a [`crate::McpServerConfig`] with
//! [`crate::ConfigScope`] supplied by the caller. URL-based ("http"/"sse")
//! entries are accepted via the `url` field.

use crate::connection::{ConfigScope, McpServerConfig};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use traits::McpTransportSpec;

/// Errors raised while parsing a `.mcp.json` file.
#[derive(Debug, thiserror::Error)]
pub enum McpJsonError {
    /// Invalid JSON.
    #[error("invalid .mcp.json: {0}")]
    Json(#[from] serde_json::Error),
    /// An entry had neither `command` nor `url` set.
    #[error("server '{0}' missing both command and url")]
    UnknownTransport(String),
}

#[derive(Debug, Deserialize)]
struct McpJsonTop {
    #[serde(default, rename = "mcpServers")]
    mcp_servers: HashMap<String, McpJsonEntry>,
}

#[derive(Debug, Deserialize)]
struct McpJsonEntry {
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: HashMap<String, String>,
    #[serde(default)]
    url: Option<String>,
    /// "http" | "sse" — only honoured when `url` is set.
    #[serde(default, rename = "type")]
    transport_type: Option<String>,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    disabled: bool,
}

/// Parse a `.mcp.json` payload (raw file contents) into a list of configs.
///
/// `scope` propagates onto every returned config so the approval policy can
/// distinguish project from user origins.
///
/// Returns `Ok(vec![])` when the input has no `mcpServers` block.
pub fn parse_mcp_json_string(
    raw: &str,
    scope: ConfigScope,
) -> Result<Vec<McpServerConfig>, McpJsonError> {
    let top: McpJsonTop = serde_json::from_str(raw)?;
    let mut out = Vec::new();
    for (name, entry) in top.mcp_servers {
        let spec = if let Some(cmd) = entry.command {
            McpTransportSpec::Stdio {
                command: cmd,
                args: entry.args,
                env: entry.env,
            }
        } else if let Some(url) = entry.url {
            match entry.transport_type.as_deref() {
                Some("sse") => McpTransportSpec::Sse {
                    url,
                    headers: entry.headers,
                    headers_helper: None,
                    oauth: None,
                },
                _ => McpTransportSpec::Http {
                    url,
                    headers: entry.headers,
                    oauth: None,
                },
            }
        } else {
            return Err(McpJsonError::UnknownTransport(name));
        };
        out.push(McpServerConfig {
            name,
            spec,
            scope,
            disabled: entry.disabled,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Load + merge `.mcp.json` configs from the project path (cwd) and the
/// user-global path. Project entries take precedence on name collision.
///
/// Missing files yield empty lists. Parse errors are logged via
/// `tracing::warn!` and the corresponding file is skipped — a malformed
/// `.mcp.json` must not break startup.
#[must_use]
pub fn load_mcp_json_with_precedence(
    project_path: &Path,
    global_path: &Path,
) -> Vec<McpServerConfig> {
    let mut by_name: HashMap<String, McpServerConfig> = HashMap::new();

    // User-global first (lower precedence).
    if let Ok(raw) = std::fs::read_to_string(global_path) {
        match parse_mcp_json_string(&raw, ConfigScope::User) {
            Ok(cfgs) => {
                for c in cfgs {
                    by_name.insert(c.name.clone(), c);
                }
            }
            Err(e) => tracing::warn!(
                error = %e,
                path = %global_path.display(),
                "skipping malformed user mcp.json"
            ),
        }
    }

    // Project overrides on name collision.
    if let Ok(raw) = std::fs::read_to_string(project_path) {
        match parse_mcp_json_string(&raw, ConfigScope::Project) {
            Ok(cfgs) => {
                for c in cfgs {
                    by_name.insert(c.name.clone(), c);
                }
            }
            Err(e) => tracing::warn!(
                error = %e,
                path = %project_path.display(),
                "skipping malformed project .mcp.json"
            ),
        }
    }

    let mut out: Vec<McpServerConfig> = by_name.into_values().collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn parse_empty_json_yields_no_servers() {
        let cfgs = parse_mcp_json_string("{}", ConfigScope::Project).unwrap();
        assert!(cfgs.is_empty());
    }

    #[test]
    fn parse_two_stdio_servers() {
        let raw = r#"{
          "mcpServers": {
            "memory":     { "command": "mcp-memory",     "args": [],         "env": {} },
            "filesystem": { "command": "mcp-filesystem", "args": ["/tmp"],   "env": {} }
          }
        }"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::Project).unwrap();
        assert_eq!(cfgs.len(), 2);
        // Sorted by name.
        assert_eq!(cfgs[0].name, "filesystem");
        assert_eq!(cfgs[1].name, "memory");
        match &cfgs[0].spec {
            McpTransportSpec::Stdio { command, args, .. } => {
                assert_eq!(command, "mcp-filesystem");
                assert_eq!(args, &vec!["/tmp".to_string()]);
            }
            other => panic!("expected Stdio, got {other:?}"),
        }
    }

    #[test]
    fn parse_http_url_transport() {
        let raw = r#"{
          "mcpServers": {
            "remote": { "url": "https://example.test/mcp" }
          }
        }"#;
        let cfgs = parse_mcp_json_string(raw, ConfigScope::User).unwrap();
        assert_eq!(cfgs.len(), 1);
        match &cfgs[0].spec {
            McpTransportSpec::Http { url, .. } => {
                assert_eq!(url, "https://example.test/mcp");
            }
            other => panic!("expected Http, got {other:?}"),
        }
    }

    #[test]
    fn missing_command_and_url_errors() {
        let raw = r#"{"mcpServers":{"bogus":{}}}"#;
        let err = parse_mcp_json_string(raw, ConfigScope::Project).unwrap_err();
        match err {
            McpJsonError::UnknownTransport(name) => assert_eq!(name, "bogus"),
            McpJsonError::Json(e) => panic!("expected UnknownTransport, got Json({e})"),
        }
    }

    #[test]
    fn project_overrides_user_on_name_collision() {
        let dir = TempDir::new().unwrap();
        let project = dir.path().join(".mcp.json");
        let global = dir.path().join("user-mcp.json");
        fs::write(&global, r#"{"mcpServers":{"x":{"command":"global-x"}}}"#).unwrap();
        fs::write(&project, r#"{"mcpServers":{"x":{"command":"project-x"}}}"#).unwrap();

        let cfgs = load_mcp_json_with_precedence(&project, &global);
        assert_eq!(cfgs.len(), 1);
        assert_eq!(cfgs[0].scope, ConfigScope::Project);
        match &cfgs[0].spec {
            McpTransportSpec::Stdio { command, .. } => assert_eq!(command, "project-x"),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn malformed_user_file_is_skipped_silently() {
        let dir = TempDir::new().unwrap();
        let project = dir.path().join(".mcp.json");
        let global = dir.path().join("user-mcp.json");
        fs::write(&global, "{ not json").unwrap();
        fs::write(&project, r#"{"mcpServers":{"y":{"command":"y"}}}"#).unwrap();
        let cfgs = load_mcp_json_with_precedence(&project, &global);
        assert_eq!(cfgs.len(), 1);
        assert_eq!(cfgs[0].name, "y");
    }
}
