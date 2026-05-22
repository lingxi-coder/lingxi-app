//! Strict-plugin-only policy.
//!
//! Host policy can lock individual component slots so that only plugin
//! sources may contribute them. A locked slot rejects user/project-level
//! settings, dynamic configs, etc. — the only acceptable source is a
//! plugin that has been admin-approved.
//!
//! See spec §15.6.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// A component category that can be locked by the strict policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[allow(missing_docs)]
pub enum PluginComponent {
    Commands,
    Agents,
    Skills,
    Hooks,
    OutputStyles,
    McpServers,
    LspServers,
    Channels,
}

/// Strict-plugin-only policy.
///
/// A component appearing in `locked` is only allowed to be contributed by
/// a plugin source — manual `claude` config, project files, etc. are
/// ignored for that slot.
pub struct StrictPluginOnlyPolicy {
    /// Components that must come from a plugin source only.
    pub locked: HashSet<PluginComponent>,
}

impl StrictPluginOnlyPolicy {
    /// Empty policy — no components are locked.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            locked: HashSet::new(),
        }
    }

    /// Return `true` when `c` is locked (plugin sources only).
    #[must_use]
    pub fn is_locked(&self, c: PluginComponent) -> bool {
        self.locked.contains(&c)
    }
}
