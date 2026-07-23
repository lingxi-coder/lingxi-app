//! Output-style data model: how the engine formats assistant output (markdown,
//! plain text, JSON stream, etc.) and how style descriptors flow in from
//! configuration files or plugins.
//!
//! See spec §21.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A registered output style.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputStyle {
    /// Canonical style name (also used as the registry key).
    pub name: String,
    /// Short user-facing description.
    pub description: String,
    /// Provenance classification.
    pub source: OutputStyleSource,
    /// Parsed YAML frontmatter from the originating file (if any).
    pub frontmatter: OutputStyleFrontmatter,
    /// Text appended to the system prompt when this style is active.
    pub system_prompt_addendum: String,
    /// Filesystem path of the originating definition, if loaded from disk.
    pub source_path: Option<PathBuf>,
}

/// YAML frontmatter shape for output-style definition files.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputStyleFrontmatter {
    /// Style name (must match the filename slug in practice).
    pub name: String,
    /// Short description.
    pub description: String,
    /// When true this style is treated as the default if multiple match.
    pub default: bool,
    /// Wire-level output format this style renders to.
    pub format: OutputFormat,
    /// Preserve the standard coding instructions when this style is active.
    #[serde(rename = "keepCodingInstructions")]
    pub keep_coding_instructions: bool,
    /// Plugin name that activates this style while that plugin is enabled.
    #[serde(rename = "force-for-plugin", skip_serializing_if = "Option::is_none")]
    pub force_for_plugin: Option<String>,
}

impl Default for OutputStyleFrontmatter {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            default: false,
            format: OutputFormat::default(),
            keep_coding_instructions: true,
            force_for_plugin: None,
        }
    }
}

/// Wire-level output formats supported by the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum OutputFormat {
    /// Markdown with code fences and headings (default).
    #[default]
    Markdown,
    /// Plain text, no markup.
    Plain,
    /// JSON-encoded events streamed line-by-line.
    JsonStream,
    /// Markdown styled for terse, minimal answers.
    Concise,
    /// Markdown styled for in-depth, pedagogical answers.
    Explanatory,
}

/// Where the style came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputStyleSource {
    /// Compiled-in default.
    Builtin,
    /// User-level configuration directory.
    User,
    /// Project-local configuration directory.
    Project,
    /// Loaded by an installed plugin.
    Plugin,
    /// Loaded from a managed (admin-controlled) source.
    Managed,
}
