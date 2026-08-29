//! Dynamic authentication headers for remote MCP transports.

use crate::{ConfigScope, McpServerConfig};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use traits::{McpError, McpHeaders, McpTransportSpec};

const HELPER_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_HELPER_STDOUT: usize = 1024 * 1024;

/// Return whether a transport has a configured dynamic headers helper.
#[must_use]
pub fn has_headers_helper(spec: &McpTransportSpec) -> bool {
    headers_helper_parts(spec).is_some()
}

/// Resolve `headersHelper` immediately before a connect attempt.
///
/// The helper runs for every connect/reconnect. Its JSON object is merged over
/// static headers, so a short-lived credential can replace a configured value.
/// Project/local helpers are rejected until the workspace trust record exists;
/// this check happens before the subprocess is created.
pub async fn resolve_headers_helper(
    config: &McpServerConfig,
) -> Result<McpTransportSpec, McpError> {
    let cwd = std::env::current_dir().map_err(|error| {
        McpError::Connection(format!(
            "headersHelper for MCP server \"{}\" could not resolve cwd: {error}",
            config.name
        ))
    })?;
    resolve_headers_helper_in(config, &cwd, None).await
}

/// Resolve `headersHelper` with the session and optional plugin execution
/// context selected by the runtime composition root.
pub async fn resolve_headers_helper_in(
    config: &McpServerConfig,
    cwd: &Path,
    plugin_root: Option<&Path>,
) -> Result<McpTransportSpec, McpError> {
    let Some((url, command)) = headers_helper_parts(&config.spec) else {
        return Ok(config.spec.clone());
    };
    ensure_helper_source_trusted(config.scope, cwd)?;

    let dynamic = run_helper(command, &config.name, url, cwd, plugin_root).await?;
    Ok(merge_dynamic_headers(&config.spec, dynamic))
}

fn headers_helper_parts(spec: &McpTransportSpec) -> Option<(&str, &str)> {
    match spec {
        McpTransportSpec::Sse {
            url,
            headers_helper: Some(helper),
            ..
        }
        | McpTransportSpec::Http {
            url,
            headers_helper: Some(helper),
            ..
        }
        | McpTransportSpec::WebSocket {
            url,
            headers_helper: Some(helper),
            ..
        } if !helper.trim().is_empty() => Some((url, helper)),
        _ => None,
    }
}

fn ensure_helper_source_trusted(scope: ConfigScope, cwd: &Path) -> Result<(), McpError> {
    if !matches!(scope, ConfigScope::Project | ConfigScope::Local) {
        return Ok(());
    }
    let trusted = migrations::global_config::global_config_path()
        .is_some_and(|path| migrations::global_config::check_has_trust_dialog_accepted(&path, cwd));
    if trusted {
        Ok(())
    } else {
        Err(McpError::Connection(
            "project/local MCP headersHelper is disabled until this workspace is trusted"
                .to_string(),
        ))
    }
}

async fn run_helper(
    command: &str,
    server_name: &str,
    server_url: &str,
    cwd: &Path,
    plugin_root: Option<&Path>,
) -> Result<McpHeaders, McpError> {
    let mut process = shell_command(command);
    process
        .current_dir(cwd)
        .env("CLAUDE_CODE_MCP_SERVER_NAME", server_name)
        .env("CLAUDE_CODE_MCP_SERVER_URL", server_url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(plugin_root) = plugin_root {
        process.env("CLAUDE_PLUGIN_ROOT", plugin_root);
    } else if let Ok(plugin_root) = std::env::var("CLAUDE_PLUGIN_ROOT") {
        process.env("CLAUDE_PLUGIN_ROOT", plugin_root);
    }

    let mut child = process.spawn().map_err(|error| {
        McpError::Connection(format!(
            "headersHelper for MCP server \"{server_name}\" failed to start: {error}"
        ))
    })?;
    let mut stdout = child.stdout.take().ok_or_else(|| {
        McpError::Connection(format!(
            "headersHelper for MCP server \"{server_name}\" failed to start: missing stdout"
        ))
    })?;
    let mut stderr = child.stderr.take().ok_or_else(|| {
        McpError::Connection(format!(
            "headersHelper for MCP server \"{server_name}\" failed to start: missing stderr"
        ))
    })?;

    let read_stdout = async {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 8192];
        loop {
            let n = stdout.read(&mut tmp).await.map_err(|error| {
                McpError::Connection(format!(
                    "headersHelper for MCP server \"{server_name}\" failed to start: {error}"
                ))
            })?;
            if n == 0 {
                break;
            }
            if buf.len().saturating_add(n) > MAX_HELPER_STDOUT {
                return Err(McpError::Connection(format!(
                    "headersHelper for MCP server \"{server_name}\" returned more than 1 MiB"
                )));
            }
            buf.extend_from_slice(&tmp[..n]);
        }
        Ok::<_, McpError>(buf)
    };
    let read_stderr = async {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 8192];
        loop {
            let n = match stderr.read(&mut tmp).await {
                Ok(n) => n,
                Err(_) => break,
            };
            if n == 0 {
                break;
            }
            let take = n.min(MAX_HELPER_STDOUT.saturating_sub(buf.len()));
            buf.extend_from_slice(&tmp[..take]);
            if buf.len() >= MAX_HELPER_STDOUT {
                // Stop retaining diagnostics at the cap, but keep draining
                // the pipe so a noisy helper cannot block before exit.
                loop {
                    match stderr.read(&mut tmp).await {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                }
                break;
            }
        }
        Ok::<_, McpError>(buf)
    };

    let collect = async {
        let (stdout_bytes, stderr_bytes) = tokio::try_join!(read_stdout, read_stderr)?;
        let status = child.wait().await.map_err(|error| {
            McpError::Connection(format!(
                "headersHelper for MCP server \"{server_name}\" failed to start: {error}"
            ))
        })?;
        Ok::<_, McpError>((status, stdout_bytes, stderr_bytes))
    };

    let (status, stdout_bytes, stderr_bytes) =
        match tokio::time::timeout(HELPER_TIMEOUT, collect).await {
            Ok(Ok(parts)) => parts,
            Ok(Err(error)) => {
                let _ = child.start_kill();
                return Err(error);
            }
            Err(_) => {
                let _ = child.start_kill();
                return Err(McpError::Connection(format!(
                    "headersHelper for MCP server \"{server_name}\" timed out after 10s"
                )));
            }
        };
    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr_bytes);
        let detail = stderr.trim();
        return Err(McpError::Connection(format!(
            "headersHelper for MCP server \"{server_name}\" exited {}{}",
            status
                .code()
                .map_or_else(|| "without a status".to_string(), |code| code.to_string()),
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        )));
    }
    parse_helper_output(&stdout_bytes, server_name)
}

#[cfg(unix)]
fn shell_command(command: &str) -> tokio::process::Command {
    let mut process = tokio::process::Command::new("/bin/sh");
    process.arg("-c").arg(command);
    process
}

#[cfg(windows)]
fn shell_command(command: &str) -> tokio::process::Command {
    let mut process = tokio::process::Command::new("cmd.exe");
    process.arg("/D").arg("/S").arg("/C").arg(command);
    process
}

fn parse_helper_output(bytes: &[u8], server_name: &str) -> Result<McpHeaders, McpError> {
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|error| {
        McpError::Connection(format!(
            "headersHelper for MCP server \"{server_name}\" returned invalid JSON: {error}"
        ))
    })?;
    let serde_json::Value::Object(object) = value else {
        return Err(McpError::Connection(format!(
            "headersHelper for MCP server \"{server_name}\" must return a JSON object"
        )));
    };
    object
        .into_iter()
        .map(|(name, value)| match value {
            serde_json::Value::String(value) => Ok((name, value)),
            _ => Err(McpError::Connection(format!(
                "headersHelper for MCP server \"{server_name}\" returned a non-string value for \"{name}\""
            ))),
        })
        .collect()
}

fn merge_dynamic_headers(spec: &McpTransportSpec, dynamic: McpHeaders) -> McpTransportSpec {
    let mut resolved = spec.clone();
    let headers = match &mut resolved {
        McpTransportSpec::Sse { headers, .. }
        | McpTransportSpec::Http { headers, .. }
        | McpTransportSpec::WebSocket { headers, .. } => headers,
        _ => return resolved,
    };
    for (name, value) in dynamic {
        headers.insert(name, value);
    }
    resolved
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Which scopes the trust gate covers is oracle `Hr` (2.1.251 @182175156):
    /// ```text
    /// function Hr(e){switch(e){
    ///   case"project": case"local": return"repo";
    ///   case"user": case"dynamic": case"enterprise": case"claudeai":
    ///   case"managed": case"agent": return"operator";
    ///   default: return e}}
    /// function Fr(e){return Hr(e.scope)==="repo" || ...}
    /// ```
    /// and `jht` only returns `missing_trust` when `isRepoResidentConfig`
    /// (= `Fr`) is set (@157454467). So exactly `project` and `local` are
    /// gated; every other scope — `dynamic` included — runs the helper
    /// without a trust record.
    ///
    /// This is load-bearing for `--mcp-config`: those entries are stamped
    /// `dynamic` (oracle `Tl={...ws,scope:"dynamic"}`, port
    /// `apps/cli/src/init.rs`), which moved them OUT of this gated set. The
    /// change is oracle-faithful, but it is a security-relevant boundary and
    /// must not drift silently — hence this pin.
    #[test]
    fn only_project_and_local_scopes_are_trust_gated() {
        let cwd = std::path::Path::new("/definitely/not/a/trusted/workspace");
        for scope in [ConfigScope::Project, ConfigScope::Local] {
            let err = ensure_helper_source_trusted(scope, cwd)
                .expect_err("repo-resident scopes must require a trust record");
            assert_eq!(
                err.to_string(),
                "connection failed: project/local MCP headersHelper is disabled until this workspace is trusted"
            );
        }
        for scope in [
            ConfigScope::User,
            ConfigScope::Dynamic,
            ConfigScope::Enterprise,
            ConfigScope::ClaudeAi,
            ConfigScope::Managed,
            ConfigScope::Agent,
        ] {
            assert!(
                ensure_helper_source_trusted(scope, cwd).is_ok(),
                "`Hr` maps {scope:?} to \"operator\", which `Fr` leaves ungated"
            );
        }
    }

    #[test]
    fn dynamic_headers_replace_static_values() {
        let spec = McpTransportSpec::Http {
            url: "https://mcp.example".into(),
            headers: McpHeaders::from_iter([
                ("Authorization".into(), "old".into()),
                ("X-Static".into(), "yes".into()),
            ]),
            headers_helper: Some("helper".into()),
            oauth: None,
        };
        let merged = merge_dynamic_headers(
            &spec,
            McpHeaders::from_iter([("Authorization".into(), "new".into())]),
        );
        let McpTransportSpec::Http { headers, .. } = merged else {
            panic!("expected http")
        };
        assert_eq!(headers["Authorization"], "new");
        assert_eq!(headers["X-Static"], "yes");
    }

    #[test]
    fn helper_output_rejects_non_string_values() {
        let error = parse_helper_output(br#"{"Authorization": 3}"#, "example").unwrap_err();
        assert!(error.to_string().contains("non-string"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn helper_receives_session_and_plugin_context() {
        let session = tempfile::tempdir().unwrap();
        let plugin = tempfile::tempdir().unwrap();
        let config = McpServerConfig {
            name: "remote".into(),
            spec: McpTransportSpec::Http {
                url: "https://mcp.example/rpc".into(),
                headers: McpHeaders::from_iter([("X-Static".into(), "old".into())]),
                headers_helper: Some(
                    r#"printf '{"X-Name":"%s","X-Url":"%s","X-Cwd":"%s","X-Plugin":"%s","X-Static":"new"}' "$CLAUDE_CODE_MCP_SERVER_NAME" "$CLAUDE_CODE_MCP_SERVER_URL" "$(pwd)" "$CLAUDE_PLUGIN_ROOT""#.into(),
                ),
                oauth: None,
            },
            scope: ConfigScope::User,
            disabled: false,
            timeout_ms: None,
            always_load: false,
            config_error: None,
        };
        let resolved = resolve_headers_helper_in(&config, session.path(), Some(plugin.path()))
            .await
            .unwrap();
        let McpTransportSpec::Http { headers, .. } = resolved else {
            panic!("expected http transport");
        };
        assert_eq!(headers["X-Name"], "remote");
        assert_eq!(headers["X-Url"], "https://mcp.example/rpc");
        assert_eq!(
            headers["X-Cwd"],
            session.path().canonicalize().unwrap().to_string_lossy()
        );
        assert_eq!(headers["X-Plugin"], plugin.path().to_string_lossy());
        assert_eq!(headers["X-Static"], "new");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn helper_kills_process_when_stdout_exceeds_cap() {
        let session = tempfile::tempdir().unwrap();
        let config = McpServerConfig {
            name: "remote".into(),
            spec: McpTransportSpec::Http {
                url: "https://mcp.example/rpc".into(),
                headers: McpHeaders::new(),
                headers_helper: Some("dd if=/dev/zero bs=1048576 count=2 2>/dev/null".into()),
                oauth: None,
            },
            scope: ConfigScope::User,
            disabled: false,
            timeout_ms: None,
            always_load: false,
            config_error: None,
        };
        let error = resolve_headers_helper_in(&config, session.path(), None)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("more than 1 MiB"), "{error}");
    }
}
