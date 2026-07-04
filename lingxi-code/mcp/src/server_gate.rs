//! Per-project MCP-server enable/disable gate (claude-code `eI()` / `rTo()` /
//! `bX` — 2.1.200+).
//!
//! claude-code stores two per-project server-gate lists in the global config
//! (`~/.claude.json` `projects[<cwd>]`, read via `wh()` → `Ot().projects[Hae()]`):
//!
//! - `enabledMcpServers` — an ALLOWLIST that applies ONLY to the builtin
//!   `computer-use` server (`bX = "computer-use"`).
//! - `disabledMcpServers` — a DENYLIST that applies to every OTHER server.
//!
//! The gate predicate is `eI()` (returns TRUE ⇒ the server is disabled and is
//! skipped at load: `if(eI(cn))return`, rendered `type:eI(x)?"disabled":"pending"`,
//! and consulted at cache-validation + auth):
//!
//! ```js
//! function Eqn(e){return Array.isArray(e)?e:[]}          // non-array tolerance
//! function rTo(e){return e===bX}                          // is builtin computer-use
//! function eI(e){let t=wh();
//!   if(rTo(e)) return !Eqn(t.enabledMcpServers).includes(e); // allowlist
//!   return Eqn(t.disabledMcpServers).includes(e)}            // denylist
//! ```
//!
//! In lingxi the per-project config lives in `~/.lingxi.json`
//! `projects.<project_key>` and is read via
//! [`migrations::global_config::get_project_config`]. Only the `computer-use`
//! server name is a wire/protocol identifier and stays verbatim (not branded).

use crate::connection::McpServerConfig;
use serde_json::{Map, Value};
use std::path::Path;

/// `bX = "computer-use"` — the single builtin MCP server subject to the
/// `enabledMcpServers` ALLOWLIST rather than the `disabledMcpServers` denylist.
/// A wire/protocol identifier: kept verbatim (NOT brand-swapped).
pub const BUILTIN_COMPUTER_USE_SERVER: &str = "computer-use";

/// `rTo(e)`: whether `name` is the builtin `computer-use` server (the only
/// server governed by the allowlist rather than the denylist).
#[must_use]
pub fn is_builtin_computer_use(name: &str) -> bool {
    name == BUILTIN_COMPUTER_USE_SERVER
}

/// `Eqn(e)`: non-array tolerance — return the JSON value as a list of strings,
/// or an empty list when it is absent OR any non-array value (matching JS
/// `Array.isArray(e)?e:[]`). Non-string array elements never match a string
/// server name via `.includes(name)`, so they are dropped here without changing
/// the predicate result.
#[must_use]
pub fn eqn_string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// `eI(e)`: whether the server named `name` is DISABLED (gated out) given the
/// project's `enabledMcpServers` / `disabledMcpServers` lists.
///
/// - Builtin `computer-use` (`rTo`): an ALLOWLIST — enabled ONLY when present in
///   `enabled`, so it is disabled iff NOT included.
/// - Every other server: a DENYLIST — disabled iff present in `disabled`.
///
/// TRUE ⇒ the server must be skipped at load (`if(eI(cn))return`) and rendered
/// as `disabled`.
#[must_use]
pub fn mcp_server_is_disabled(name: &str, enabled: &[String], disabled: &[String]) -> bool {
    if is_builtin_computer_use(name) {
        // Allowlist: disabled unless explicitly enabled.
        !enabled.iter().any(|s| s == name)
    } else {
        // Denylist: disabled iff explicitly disabled.
        disabled.iter().any(|s| s == name)
    }
}

/// Read `(enabledMcpServers, disabledMcpServers)` from a project-config map
/// (`~/.lingxi.json` `projects.<key>`), applying the `Eqn` non-array tolerance
/// to each list.
#[must_use]
pub fn read_gate_lists(project_cfg: &Map<String, Value>) -> (Vec<String>, Vec<String>) {
    (
        eqn_string_array(project_cfg.get("enabledMcpServers")),
        eqn_string_array(project_cfg.get("disabledMcpServers")),
    )
}

/// Apply the per-project MCP-server gate to a loaded server list by MARKING each
/// gated server `disabled` (claude-code `if(eI(cn))return` at the load path).
///
/// A disabled server is NOT connected — [`crate::registry::McpRegistry::connect_all`]
/// seeds it as `Disconnected` so `/mcp` still lists it (matching the oracle's
/// `type:eI(x)?"disabled":"pending"` rendering) — while all other servers connect
/// normally. The lists come from `~/.lingxi.json` `projects.<cwd_key>`
/// (`enabledMcpServers` allowlist for `computer-use`, `disabledMcpServers`
/// denylist for the rest).
///
/// A missing/unreadable global config or project entry yields empty lists, so
/// only the builtin `computer-use` server (never allowlisted) is gated off — a
/// byte-faithful match for `wh()` returning the empty project record.
pub fn apply_project_server_gate(
    servers: &mut [McpServerConfig],
    global_config_path: &Path,
    cwd: &Path,
) {
    let key = migrations::global_config::project_path_for_config(cwd);
    let project_cfg =
        migrations::global_config::get_project_config(global_config_path, &key).unwrap_or_default();
    let (enabled, disabled) = read_gate_lists(&project_cfg);
    for server in servers.iter_mut() {
        if mcp_server_is_disabled(&server.name, &enabled, &disabled) {
            server.disabled = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eqn_tolerates_non_arrays() {
        // Absent ⇒ [].
        assert!(eqn_string_array(None).is_empty());
        // Non-array JSON values ⇒ [] (Array.isArray(e)?e:[]).
        assert!(eqn_string_array(Some(&serde_json::json!("srv"))).is_empty());
        assert!(eqn_string_array(Some(&serde_json::json!(42))).is_empty());
        assert!(eqn_string_array(Some(&serde_json::json!({"a": 1}))).is_empty());
        assert!(eqn_string_array(Some(&serde_json::json!(null))).is_empty());
        // A genuine array of strings passes through.
        assert_eq!(
            eqn_string_array(Some(&serde_json::json!(["a", "b"]))),
            vec!["a".to_string(), "b".to_string()]
        );
        // Non-string array elements are dropped (they never match a server name).
        assert_eq!(
            eqn_string_array(Some(&serde_json::json!(["a", 1, true, "b"]))),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn builtin_computer_use_is_allowlisted() {
        // computer-use is DISABLED unless it appears in enabledMcpServers.
        assert!(mcp_server_is_disabled("computer-use", &[], &[]));
        // Present in the denylist is irrelevant for the builtin — only the
        // allowlist governs it.
        assert!(mcp_server_is_disabled(
            "computer-use",
            &[],
            &["computer-use".to_string()]
        ));
        // Explicitly enabled ⇒ NOT disabled.
        assert!(!mcp_server_is_disabled(
            "computer-use",
            &["computer-use".to_string()],
            &[]
        ));
        // Enabled wins even if it is ALSO (nonsensically) in the denylist —
        // eI() never consults the denylist for the builtin.
        assert!(!mcp_server_is_disabled(
            "computer-use",
            &["computer-use".to_string()],
            &["computer-use".to_string()]
        ));
    }

    #[test]
    fn other_servers_are_denylisted() {
        // A normal server is ENABLED by default (empty denylist).
        assert!(!mcp_server_is_disabled("sentry", &[], &[]));
        // Being in the ALLOWLIST does nothing for a non-builtin server — only
        // the denylist governs it.
        assert!(!mcp_server_is_disabled("sentry", &["sentry".to_string()], &[]));
        // Present in the denylist ⇒ disabled.
        assert!(mcp_server_is_disabled(
            "sentry",
            &[],
            &["sentry".to_string()]
        ));
        // Allowlisted AND denylisted ⇒ still disabled (denylist governs it).
        assert!(mcp_server_is_disabled(
            "sentry",
            &["sentry".to_string()],
            &["sentry".to_string()]
        ));
    }

    #[test]
    fn read_gate_lists_extracts_both_keys_with_tolerance() {
        let cfg: Map<String, Value> = serde_json::from_value(serde_json::json!({
            "enabledMcpServers": ["computer-use"],
            "disabledMcpServers": ["sentry", "linear"],
            "mcpServers": {}, // unrelated key ignored
        }))
        .unwrap();
        let (enabled, disabled) = read_gate_lists(&cfg);
        assert_eq!(enabled, vec!["computer-use".to_string()]);
        assert_eq!(
            disabled,
            vec!["sentry".to_string(), "linear".to_string()]
        );

        // Non-array values ⇒ empty lists (Eqn tolerance), not a panic.
        let cfg2: Map<String, Value> = serde_json::from_value(serde_json::json!({
            "enabledMcpServers": "computer-use",
            "disabledMcpServers": 3,
        }))
        .unwrap();
        let (e2, d2) = read_gate_lists(&cfg2);
        assert!(e2.is_empty());
        assert!(d2.is_empty());

        // Wholly absent keys ⇒ empty lists.
        let (e3, d3) = read_gate_lists(&Map::new());
        assert!(e3.is_empty());
        assert!(d3.is_empty());
    }

    #[test]
    fn apply_gate_marks_disabled_servers() {
        use crate::connection::ConfigScope;
        use std::collections::HashMap;
        use traits::McpTransportSpec;

        let stdio = |name: &str| McpServerConfig {
            name: name.to_string(),
            spec: McpTransportSpec::Stdio {
                command: "srv".into(),
                args: vec![],
                env: HashMap::new(),
            },
            scope: ConfigScope::Project,
            disabled: false,
        };

        // Write a global config with a denylist entry for one server.
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join("proj");
        std::fs::create_dir_all(&cwd).unwrap();
        let key = migrations::global_config::project_path_for_config(&cwd);
        let global = dir.path().join(".lingxi.json");
        let contents = serde_json::json!({
            "projects": {
                key: {
                    "disabledMcpServers": ["sentry"],
                    // computer-use NOT in enabledMcpServers ⇒ stays disabled.
                }
            }
        });
        std::fs::write(&global, serde_json::to_vec(&contents).unwrap()).unwrap();

        let mut servers = vec![stdio("sentry"), stdio("linear"), stdio("computer-use")];
        apply_project_server_gate(&mut servers, &global, &cwd);

        // Denylisted server is disabled.
        assert!(servers.iter().find(|s| s.name == "sentry").unwrap().disabled);
        // Un-listed normal server is untouched (enabled).
        assert!(!servers.iter().find(|s| s.name == "linear").unwrap().disabled);
        // Builtin computer-use with no allowlist entry is disabled.
        assert!(
            servers
                .iter()
                .find(|s| s.name == "computer-use")
                .unwrap()
                .disabled
        );
    }

    #[test]
    fn apply_gate_missing_config_gates_only_builtin() {
        use crate::connection::ConfigScope;
        use std::collections::HashMap;
        use traits::McpTransportSpec;

        let stdio = |name: &str| McpServerConfig {
            name: name.to_string(),
            spec: McpTransportSpec::Stdio {
                command: "srv".into(),
                args: vec![],
                env: HashMap::new(),
            },
            scope: ConfigScope::Project,
            disabled: false,
        };

        let dir = tempfile::tempdir().unwrap();
        // No global config file at all ⇒ empty gate lists.
        let global = dir.path().join(".lingxi.json");
        let mut servers = vec![stdio("sentry"), stdio("computer-use")];
        apply_project_server_gate(&mut servers, &global, dir.path());

        // A normal server stays enabled when there is no denylist.
        assert!(!servers.iter().find(|s| s.name == "sentry").unwrap().disabled);
        // The builtin is disabled (never allowlisted).
        assert!(
            servers
                .iter()
                .find(|s| s.name == "computer-use")
                .unwrap()
                .disabled
        );
    }
}
