//! Agent frontmatter `mcpServers` → scoped [`mcp::McpServerConfig`]s.
//!
//! Port of claude `agentMcpSpecsToScopedConfigs` (minified `obs`,
//! 2.1.220 @231497090): the conversion the main-thread agent merge (`FWt`)
//! runs before folding an agent's inline servers into the session's dynamic
//! MCP config. String (by-name) entries, multi-key records, reserved names and
//! internal-only IDE transports are skipped — each with claude's exact debug
//! log line — and every surviving config is stamped `scope:"agent"`
//! ([`mcp::ConfigScope::Agent`]).

use crate::definition::{AgentDefinition, AgentMcpServerSpec, AgentSource};

/// Convert `def`'s frontmatter `mcpServers` into agent-scoped
/// [`mcp::McpServerConfig`]s (claude `obs`).
///
/// `strict_plugin_only_mcp` mirrors `Y0("mcp")` — the managed
/// `strictPluginOnlyCustomization` lock on the MCP slot. When set, agents from
/// non-plugin-trusted sources (`wke`: plugin / policySettings / built-in /
/// bundled) contribute NO servers. LingXi's composition root never populates
/// the strict policy today (`StrictPluginOnlyPolicy::empty()`), so production
/// callers pass `false`; the parameter keeps the gate 1:1 and testable.
///
/// Skip rules, in claude's order per entry:
/// - string entry (`ByName`) — silent skip;
/// - record with != 1 key — `Invalid MCP server spec: expected exactly one key`;
/// - reserved server name (`BIt`) — `Skipping reserved MCP server name …`;
/// - `type: sse-ide` / `ws-ide` — `Skipping internal-only MCP transport …`;
/// - anything else is built via [`mcp::build_server_from_json_entry`] (the
///   `.mcp.json` entry shape — `{...i, scope:"agent"}`); a body that fails
///   validation is logged + skipped by that builder.
#[must_use]
pub fn agent_mcp_specs_to_scoped_configs(
    def: &AgentDefinition,
    strict_plugin_only_mcp: bool,
) -> Vec<mcp::McpServerConfig> {
    if def.mcp_servers.is_empty() {
        return Vec::new();
    }
    if strict_plugin_only_mcp && !plugin_trusted_source(def.source) {
        tracing::warn!(
            "[Agent: {}] Skipping frontmatter MCP servers: strictPluginOnlyCustomization locks MCP to plugin-only (agent source: {})",
            def.agent_type,
            source_label(def.source)
        );
        return Vec::new();
    }
    let mut out: Vec<mcp::McpServerConfig> = Vec::new();
    for spec in &def.mcp_servers {
        let record = match spec {
            // claude `typeof r === "string"` → by-name entries are resolved by
            // the host, never materialized here.
            AgentMcpServerSpec::ByName(_) => continue,
            AgentMcpServerSpec::Record(map) => map,
        };
        if record.len() != 1 {
            tracing::warn!(
                "[Agent: {}] Invalid MCP server spec: expected exactly one key",
                def.agent_type
            );
            continue;
        }
        let (name, raw) = record
            .iter()
            .next()
            .expect("record.len() == 1 guarantees one entry");
        if mcp::normalization::is_reserved_mcp_server_name(name) {
            tracing::warn!(
                "[Agent: {}] Skipping reserved MCP server name '{}' in frontmatter",
                def.agent_type,
                name
            );
            continue;
        }
        if let Some(ty @ ("sse-ide" | "ws-ide")) = raw.get("type").and_then(|v| v.as_str()) {
            tracing::warn!(
                "[Agent: {}] Skipping internal-only MCP transport '{}' for '{}' in frontmatter",
                def.agent_type,
                ty,
                name
            );
            continue;
        }
        // `t[o] = {...i, scope: "agent"}` — the body shares the `.mcp.json`
        // entry shape; an invalid body is logged + dropped by the builder.
        // A plain object assignment, so a name declared twice in one list is
        // LAST-wins and keeps its original key position; the `Vec` stands in
        // for `t`'s insertion order.
        if let Some(cfg) = mcp::build_server_from_json_entry(name, raw, mcp::ConfigScope::Agent) {
            match out.iter_mut().find(|c| c.name == cfg.name) {
                Some(slot) => *slot = cfg,
                None => out.push(cfg),
            }
        }
    }
    out
}

/// claude `wke`: sources exempt from the strict plugin-only MCP lock —
/// `new Set(["plugin","policySettings","built-in","builtin","bundled"])`.
pub(crate) fn plugin_trusted_source(source: AgentSource) -> bool {
    matches!(
        source,
        AgentSource::Plugin | AgentSource::PolicySettings | AgentSource::BuiltIn
    )
}

/// Human label for the strict-lock debug line (claude interpolates the raw
/// source string; ours maps the enum back to claude's wire names).
fn source_label(source: AgentSource) -> &'static str {
    match source {
        AgentSource::BuiltIn => "built-in",
        AgentSource::UserDefined => "userSettings",
        AgentSource::Project => "projectSettings",
        AgentSource::Plugin => "plugin",
        AgentSource::PolicySettings => "policySettings",
        AgentSource::Flag => "flagSettings",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def_with_specs(specs: Vec<AgentMcpServerSpec>, source: AgentSource) -> AgentDefinition {
        let mut def = crate::catalog::parse_agent_from_json(
            "tester",
            &serde_json::json!({"description": "d", "prompt": "p"}),
            source,
        )
        .expect("minimal JSON agent parses");
        def.mcp_servers = specs;
        def
    }

    fn record(name: &str, body: serde_json::Value) -> AgentMcpServerSpec {
        let mut map = serde_json::Map::new();
        map.insert(name.to_string(), body);
        AgentMcpServerSpec::Record(map)
    }

    #[test]
    fn empty_specs_yield_no_configs() {
        let def = def_with_specs(vec![], AgentSource::Project);
        assert!(agent_mcp_specs_to_scoped_configs(&def, false).is_empty());
    }

    #[test]
    fn by_name_entries_are_skipped() {
        let def = def_with_specs(
            vec![AgentMcpServerSpec::ByName("slack".into())],
            AgentSource::Project,
        );
        assert!(agent_mcp_specs_to_scoped_configs(&def, false).is_empty());
    }

    #[test]
    fn inline_stdio_server_gets_agent_scope() {
        let def = def_with_specs(
            vec![record(
                "docs",
                serde_json::json!({"command": "npx", "args": ["-y", "docs-mcp"]}),
            )],
            AgentSource::Project,
        );
        let cfgs = agent_mcp_specs_to_scoped_configs(&def, false);
        assert_eq!(cfgs.len(), 1);
        assert_eq!(cfgs[0].name, "docs");
        assert_eq!(cfgs[0].scope, mcp::ConfigScope::Agent);
        assert!(matches!(
            &cfgs[0].spec,
            traits::McpTransportSpec::Stdio { command, .. } if command == "npx"
        ));
    }

    #[test]
    fn inline_http_server_parses_url_transport() {
        let def = def_with_specs(
            vec![record(
                "remote",
                serde_json::json!({"type": "http", "url": "https://mcp.example/api"}),
            )],
            AgentSource::UserDefined,
        );
        let cfgs = agent_mcp_specs_to_scoped_configs(&def, false);
        assert_eq!(cfgs.len(), 1);
        assert!(matches!(
            &cfgs[0].spec,
            traits::McpTransportSpec::Http { url, .. } if url == "https://mcp.example/api"
        ));
    }

    #[test]
    fn multi_key_record_is_skipped_entirely() {
        // claude obs: `Object.entries(r).length !== 1` → warn + skip; NEITHER
        // server materializes.
        let mut map = serde_json::Map::new();
        map.insert("a".into(), serde_json::json!({"command": "x"}));
        map.insert("b".into(), serde_json::json!({"command": "y"}));
        let def = def_with_specs(vec![AgentMcpServerSpec::Record(map)], AgentSource::Project);
        assert!(agent_mcp_specs_to_scoped_configs(&def, false).is_empty());
    }

    #[test]
    fn reserved_names_are_skipped() {
        for reserved in ["computer-use", "workspace", "claude-in-chrome"] {
            let def = def_with_specs(
                vec![record(reserved, serde_json::json!({"command": "x"}))],
                AgentSource::Project,
            );
            assert!(
                agent_mcp_specs_to_scoped_configs(&def, false).is_empty(),
                "reserved name {reserved} must be skipped"
            );
        }
    }

    #[test]
    fn internal_only_ide_transports_are_skipped() {
        for ty in ["sse-ide", "ws-ide"] {
            let def = def_with_specs(
                vec![record(
                    "ide",
                    serde_json::json!({"type": ty, "url": "http://127.0.0.1:1"}),
                )],
                AgentSource::Project,
            );
            assert!(
                agent_mcp_specs_to_scoped_configs(&def, false).is_empty(),
                "transport {ty} must be skipped"
            );
        }
    }

    #[test]
    fn invalid_body_is_dropped_but_siblings_survive() {
        let def = def_with_specs(
            vec![
                record("broken", serde_json::json!({"nope": true})),
                record("ok", serde_json::json!({"command": "x"})),
            ],
            AgentSource::Project,
        );
        let cfgs = agent_mcp_specs_to_scoped_configs(&def, false);
        assert_eq!(cfgs.len(), 1);
        assert_eq!(cfgs[0].name, "ok");
    }

    #[test]
    fn duplicate_names_are_last_wins_and_keep_their_position() {
        // `obs` accumulates into an OBJECT (`t[o] = {...i, scope:"agent"}`), so
        // the second `docs` overwrites the first — and JS keeps the key at its
        // original insertion position, ahead of `other`.
        let def = def_with_specs(
            vec![
                record("docs", serde_json::json!({"command": "first"})),
                record("other", serde_json::json!({"command": "x"})),
                record("docs", serde_json::json!({"command": "second"})),
            ],
            AgentSource::Project,
        );
        let cfgs = agent_mcp_specs_to_scoped_configs(&def, false);
        assert_eq!(cfgs.len(), 2, "one config per name");
        assert_eq!(cfgs[0].name, "docs");
        assert_eq!(cfgs[1].name, "other");
        assert!(
            matches!(&cfgs[0].spec, traits::McpTransportSpec::Stdio { command, .. } if command == "second"),
            "the LAST entry for a name wins"
        );
    }

    #[test]
    fn strict_plugin_only_blocks_untrusted_sources_only() {
        let specs = || vec![record("docs", serde_json::json!({"command": "x"}))];
        // Untrusted (user/project/flag) sources: locked out.
        for src in [
            AgentSource::UserDefined,
            AgentSource::Project,
            AgentSource::Flag,
        ] {
            let def = def_with_specs(specs(), src);
            assert!(
                agent_mcp_specs_to_scoped_configs(&def, true).is_empty(),
                "{src:?} must be locked under strictPluginOnlyCustomization"
            );
        }
        // wke-trusted sources pass the lock.
        for src in [
            AgentSource::Plugin,
            AgentSource::PolicySettings,
            AgentSource::BuiltIn,
        ] {
            let def = def_with_specs(specs(), src);
            assert_eq!(
                agent_mcp_specs_to_scoped_configs(&def, true).len(),
                1,
                "{src:?} is wke-exempt from the strict lock"
            );
        }
    }
}
