// lingxi-core/crates/core/src/settings/tracer.rs
//! Provenance recorder for `/doctor` (M6).
//!
//! Task 9 wires this up; Task 8 only needs the placeholder type.

use serde::Serialize;
use std::collections::BTreeMap;

/// Per-field source provenance. Filled in by Task 9.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ProvenanceTrace {
    /// Map from settings field name → the source that decided the final value.
    pub by_field: BTreeMap<String, Source>,
}

/// Where a field's effective value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// `LINGXI_*` / `CLAUDE_CODE_*` / `CLAUDE_*` env var.
    Env,
    /// `~/.claude/settings.json`.
    User,
    /// `<project_dir>/.claude/settings.json`.
    Project,
    /// Built-in defaults baseline.
    Defaults,
}
