//! Markdown frontmatter parsing for skill files.
//!
//! Accepts files starting with a `---` YAML block followed by a blank line and
//! the markdown body. Missing frontmatter falls back to default values.

use crate::model::{LoadedFrom, Skill, SkillFrontmatter, SkillSource};
use std::path::PathBuf;
use thiserror::Error;

/// Errors produced while loading a skill from disk.
#[derive(Debug, Clone, Error)]
pub enum SkillLoadError {
    /// Frontmatter could not be parsed (malformed YAML or missing terminator).
    #[error("parse failed: {0}")]
    Parse(String),
}

/// Parse a raw skill markdown string into a [`Skill`].
///
/// `raw` may optionally start with a `---`-delimited YAML frontmatter block
/// followed by `\n---\n` and the markdown body. When no frontmatter is present
/// a default [`SkillFrontmatter`] is used and the whole input is treated as
/// content.
pub fn parse_skill_markdown(
    raw: &str,
    file_path: PathBuf,
    source: SkillSource,
    loaded_from: LoadedFrom,
) -> Result<Skill, SkillLoadError> {
    let (fm, body): (SkillFrontmatter, String) = if let Some(rest) = raw.strip_prefix("---") {
        let end = rest
            .find("\n---\n")
            .ok_or_else(|| SkillLoadError::Parse("unterminated".into()))?;
        let yaml = &rest[..end];
        let body = &rest[end + 5..];
        let fm: SkillFrontmatter =
            serde_yaml::from_str(yaml).map_err(|e| SkillLoadError::Parse(e.to_string()))?;
        (fm, body.trim_start().to_string())
    } else {
        (SkillFrontmatter::default(), raw.to_string())
    };

    Ok(Skill {
        name: fm.name.clone(),
        description: fm.description.clone(),
        frontmatter: fm,
        content: body,
        source,
        loaded_from,
        plugin_id: None,
        file_path,
    })
}
