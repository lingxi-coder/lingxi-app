//! Plugin agent frontmatter validation (spec D2).
//!
//! Reject plugin-provided agent files that try to escalate privilege via
//! frontmatter. Privileges must come from the manifest at install/enable
//! time — never from a file the plugin author can mint at will.

use thiserror::Error;

/// Failure modes for [`validate_plugin_agent_frontmatter`].
#[derive(Debug, Clone, Error)]
pub enum AgentValidationError {
    /// Frontmatter declares `permission_mode`.
    #[error("agent frontmatter cannot set permission_mode in plugin context")]
    PermissionModeForbidden,
    /// Frontmatter declares `hooks:`.
    #[error("agent frontmatter cannot declare hooks in plugin context")]
    HooksForbidden,
}

/// Validate frontmatter YAML for a plugin agent file.
///
/// Returns an error if any privilege-related field is present. Today the
/// check is a textual substring scan — sufficient because the engine
/// rejects every match and the plugin author would have to typo around
/// the check on purpose.
pub fn validate_plugin_agent_frontmatter(yaml: &str) -> Result<(), AgentValidationError> {
    if yaml.contains("permission_mode") {
        return Err(AgentValidationError::PermissionModeForbidden);
    }
    if yaml.contains("hooks:") {
        return Err(AgentValidationError::HooksForbidden);
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
    fn clean_frontmatter_passes() {
        let yaml = "name: x\ndescription: y\ntools: ['Read']\n";
        assert!(validate_plugin_agent_frontmatter(yaml).is_ok());
    }

    #[test]
    fn mcp_servers_do_not_reject_whole_plugin_agent() {
        let yaml = "name: x\ndescription: y\nmcpServers:\n  - docs:\n      command: docs-mcp\n";
        assert!(validate_plugin_agent_frontmatter(yaml).is_ok());
    }
}
