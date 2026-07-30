//! Plugin agent frontmatter validation (spec D2).
//!
//! Reject plugin-provided agent files that try to escalate privilege via
//! frontmatter. Privileges must come from the manifest at install/enable
//! time — never from a file the plugin author can mint at will.

use thiserror::Error;

/// Failure modes for [`validate_plugin_agent_frontmatter`].
#[derive(Debug, Clone, Error)]
pub enum AgentValidationError {
    /// Frontmatter declares `permissionMode` (or its legacy snake-case form).
    #[error("agent frontmatter cannot set permissionMode in plugin context")]
    PermissionModeForbidden,
    /// Frontmatter declares `hooks`.
    #[error("agent frontmatter cannot declare hooks in plugin context")]
    HooksForbidden,
    /// Frontmatter is not valid YAML.
    #[error("invalid agent frontmatter: {0}")]
    InvalidFrontmatter(String),
}

/// Validate frontmatter YAML for a plugin agent file.
///
/// Returns an error if any privilege-related top-level field is present.
/// Parsing the mapping is security-sensitive: the agent catalog accepts the
/// camel-case `permissionMode` key, and textual scans can both miss valid YAML
/// spellings and reject harmless comments or values.
pub fn validate_plugin_agent_frontmatter(yaml: &str) -> Result<(), AgentValidationError> {
    let value: serde_yaml::Value = serde_yaml::from_str(yaml)
        .map_err(|error| AgentValidationError::InvalidFrontmatter(error.to_string()))?;
    let Some(mapping) = value.as_mapping() else {
        return Err(AgentValidationError::InvalidFrontmatter(
            "expected a top-level mapping".to_owned(),
        ));
    };

    for key in mapping.keys().filter_map(serde_yaml::Value::as_str) {
        match key {
            "permissionMode" | "permission_mode" => {
                return Err(AgentValidationError::PermissionModeForbidden);
            }
            "hooks" => return Err(AgentValidationError::HooksForbidden),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_permission_mode() {
        let yaml = "name: x\npermission_mode: bypassPermissions\n";
        assert!(matches!(
            validate_plugin_agent_frontmatter(yaml),
            Err(AgentValidationError::PermissionModeForbidden)
        ));
    }

    #[test]
    fn rejects_catalog_permission_mode_spelling() {
        let yaml = "name: x\npermissionMode: bypassPermissions\n";
        assert!(matches!(
            validate_plugin_agent_frontmatter(yaml),
            Err(AgentValidationError::PermissionModeForbidden)
        ));
    }

    #[test]
    fn rejects_hooks_with_yaml_spacing() {
        let yaml = "name: x\nhooks : {}\n";
        assert!(matches!(
            validate_plugin_agent_frontmatter(yaml),
            Err(AgentValidationError::HooksForbidden)
        ));
    }

    #[test]
    fn clean_frontmatter_passes() {
        let yaml = "name: x\ndescription: y\ntools: ['Read']\n";
        assert!(validate_plugin_agent_frontmatter(yaml).is_ok());
    }

    #[test]
    fn privilege_names_in_comments_and_values_are_allowed() {
        let yaml =
            "name: x\n# permissionMode: bypassPermissions\ndescription: 'hooks: are documented'\n";
        assert!(validate_plugin_agent_frontmatter(yaml).is_ok());
    }

    #[test]
    fn nested_privilege_keys_do_not_trigger_top_level_gate() {
        let yaml = "name: x\nmetadata:\n  permissionMode: bypassPermissions\n  hooks:\n    preToolUse: []\n";
        assert!(validate_plugin_agent_frontmatter(yaml).is_ok());
    }

    #[test]
    fn malformed_frontmatter_fails_closed() {
        let yaml = "name: [unterminated\n";
        assert!(matches!(
            validate_plugin_agent_frontmatter(yaml),
            Err(AgentValidationError::InvalidFrontmatter(_))
        ));
    }

    #[test]
    fn non_mapping_frontmatter_fails_closed() {
        let yaml = "- name\n- x\n";
        assert!(matches!(
            validate_plugin_agent_frontmatter(yaml),
            Err(AgentValidationError::InvalidFrontmatter(_))
        ));
    }

    #[test]
    fn mcp_servers_do_not_reject_whole_plugin_agent() {
        let yaml = "name: x\ndescription: y\nmcpServers:\n  - docs:\n      command: docs-mcp\n";
        assert!(validate_plugin_agent_frontmatter(yaml).is_ok());
    }
}
