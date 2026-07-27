//! User-approval policy for project-scoped MCP servers.

use crate::connection::ConfigScope;
use std::collections::HashSet;

/// Persistent approval state for MCP server configs.
///
/// Only `Project`-scoped servers require explicit approval by default;
/// `Local`, `User`, `Enterprise`, and `Managed` servers are
/// pre-approved.
pub struct McpApprovalPolicy {
    /// True when even `Project`-scoped servers should be auto-approved.
    pub project_servers_require_approval: bool,
    /// Server names the user has explicitly approved.
    pub approved: HashSet<String>,
    /// Server names the user has explicitly rejected.
    pub rejected: HashSet<String>,
}

impl Default for McpApprovalPolicy {
    fn default() -> Self {
        Self::new()
    }
}

impl McpApprovalPolicy {
    /// Build an empty policy that requires approval for project servers.
    #[must_use]
    pub fn new() -> Self {
        Self {
            project_servers_require_approval: true,
            approved: HashSet::new(),
            rejected: HashSet::new(),
        }
    }
}

/// Result of consulting an [`McpApprovalPolicy`] for a server config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalStatus {
    /// Cleared to connect.
    Approved,
    /// User has rejected this server.
    Rejected,
    /// Awaiting user decision.
    PendingApproval,
}

impl McpApprovalPolicy {
    /// Decide whether the server with `name` and `scope` may connect.
    #[must_use]
    pub fn is_approved(&self, name: &str, scope: ConfigScope) -> ApprovalStatus {
        if self.rejected.contains(name) {
            return ApprovalStatus::Rejected;
        }
        if self.approved.contains(name) {
            return ApprovalStatus::Approved;
        }
        match scope {
            // `Agent`: frontmatter servers are merged like flag-supplied dynamic
            // configs but are NEVER project-approval-gated (claude's approval
            // prompt covers `.mcp.json` project servers; `FWt` merges agent
            // servers straight into `dynamicMcpConfig` after the enterprise
            // filter).
            ConfigScope::Local
            | ConfigScope::User
            | ConfigScope::Enterprise
            | ConfigScope::Managed
            | ConfigScope::Agent => ApprovalStatus::Approved,
            ConfigScope::Project | ConfigScope::Dynamic | ConfigScope::ClaudeAi => {
                ApprovalStatus::PendingApproval
            }
        }
    }
}
