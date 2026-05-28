//! Agent catalog loader (M6-07).
//!
//! Reads markdown files from one or more `agents/` directories, parses the
//! YAML frontmatter, and projects each file into an
//! [`crate::definition::AgentDefinition`].
//!
//! Frontmatter shape (claude-code compatible — subset relevant to v0.7.0):
//! ```yaml
//! ---
//! name: reviewer
//! description: Reviews code for security and correctness.
//! tools: [Read, Grep, Bash]
//! model: sonnet
//! ---
//! Body of the system prompt goes here.
//! ```
//!
//! Files without a `---`-delimited YAML frontmatter block, or with
//! malformed YAML, are skipped at the loader level (logged via
//! `tracing::warn!`). The exposed [`parse_agent_markdown`] returns a
//! typed error so unit tests can assert on the failure mode.

use crate::definition::{
    AgentDefinition, AgentModel, AgentPermissionMode, AgentSource, AgentToolPolicy,
};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Errors raised while loading an agent file.
#[derive(Debug, thiserror::Error)]
pub enum AgentLoadError {
    /// File could not be read.
    #[error("read {path}: {source}")]
    Io {
        /// Offending path.
        path: PathBuf,
        /// Underlying io error.
        #[source]
        source: std::io::Error,
    },
    /// Frontmatter was missing or its `---` terminator was not found.
    #[error("no valid frontmatter in {0}")]
    NoFrontmatter(PathBuf),
    /// Frontmatter YAML failed to deserialize.
    #[error("deserialize frontmatter {path}: {source}")]
    Yaml {
        /// Offending path.
        path: PathBuf,
        /// Underlying yaml error.
        #[source]
        source: serde_yaml::Error,
    },
}

/// Subset of an agent's YAML frontmatter we read at v0.7.0.
#[derive(Debug, Default, Deserialize)]
struct Frontmatter {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default)]
    model: Option<String>,
}

/// Parse a single agent markdown buffer.
///
/// `source` and `base_dir` are supplied by the caller (so the loader can
/// tag files coming from `~/.claude/agents/` differently from project
/// files). `path_for_error` is used purely for error messages.
///
/// Frontmatter format: leading `---\n…\n---\n` (followed by the markdown
/// body). The body is stored as `system_prompt`.
pub fn parse_agent_markdown(
    raw: &str,
    source: AgentSource,
    base_dir: PathBuf,
    path_for_error: &Path,
) -> Result<AgentDefinition, AgentLoadError> {
    let rest = raw
        .strip_prefix("---")
        .ok_or_else(|| AgentLoadError::NoFrontmatter(path_for_error.to_path_buf()))?;
    // Accept either `\n---\n` or `\n---` at EOF.
    let (yaml, body) = if let Some(idx) = rest.find("\n---\n") {
        (&rest[..idx], rest[idx + 5..].trim_start().to_string())
    } else if let Some(idx) = rest.find("\n---") {
        // EOF terminator (no trailing newline after the closing ---).
        let after = idx + 4;
        let body = if after >= rest.len() {
            String::new()
        } else {
            rest[after..].trim_start().to_string()
        };
        (&rest[..idx], body)
    } else {
        return Err(AgentLoadError::NoFrontmatter(path_for_error.to_path_buf()));
    };

    let fm: Frontmatter = serde_yaml::from_str(yaml).map_err(|e| AgentLoadError::Yaml {
        path: path_for_error.to_path_buf(),
        source: e,
    })?;

    let name = fm.name.unwrap_or_else(|| {
        // Fallback: filename stem.
        path_for_error
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("agent")
            .to_string()
    });
    let description = fm.description.unwrap_or_default();
    let tools_policy = if fm.tools.is_empty() {
        AgentToolPolicy::All {
            use_exact_tools: false,
        }
    } else {
        AgentToolPolicy::Explicit(fm.tools.clone())
    };
    let model = fm.model.map_or(AgentModel::Inherit, AgentModel::Alias);

    Ok(AgentDefinition {
        agent_type: name,
        when_to_use: description,
        tools: tools_policy,
        max_turns: 100,
        model,
        permission_mode: AgentPermissionMode::Bubble,
        source,
        base_dir,
        system_prompt: if body.is_empty() { None } else { Some(body) },
        mcp_servers: Vec::new(),
        frontmatter_hooks: Vec::new(),
        icon: None,
        allowed_tools: fm.tools,
        worktree_requirement: None,
    })
}

/// Load every `*.md` agent file under each path in `paths`, in order.
///
/// Files with no frontmatter or invalid YAML are logged at `warn!` and
/// skipped. On `agent_type` collision, **later paths win** — pass the
/// global path FIRST and the project path SECOND so project agents
/// override user-globals.
///
/// The returned list is sorted alphabetically by `agent_type` for stable
/// display order in `/agents`.
pub async fn load_agents_from_dirs(paths: &[(PathBuf, AgentSource)]) -> Vec<AgentDefinition> {
    use std::collections::HashMap;
    let mut by_name: HashMap<String, AgentDefinition> = HashMap::new();
    for (dir, source) in paths {
        // missing dir = empty contribution
        let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let p = entry.path();
            if p.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            let raw = match tokio::fs::read_to_string(&p).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        path = %p.display(),
                        "skipping unreadable agent file"
                    );
                    continue;
                }
            };
            match parse_agent_markdown(&raw, *source, dir.clone(), &p) {
                Ok(def) => {
                    by_name.insert(def.agent_type.clone(), def);
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        path = %p.display(),
                        "skipping malformed agent file"
                    );
                }
            }
        }
    }
    let mut out: Vec<AgentDefinition> = by_name.into_values().collect();
    out.sort_by(|a, b| a.agent_type.cmp(&b.agent_type));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn parse_minimal_frontmatter() {
        let raw = "---\nname: reviewer\ndescription: review code\n---\nBody";
        let def = parse_agent_markdown(
            raw,
            AgentSource::UserDefined,
            PathBuf::from("/tmp"),
            Path::new("reviewer.md"),
        )
        .unwrap();
        assert_eq!(def.agent_type, "reviewer");
        assert_eq!(def.when_to_use, "review code");
        assert_eq!(def.system_prompt.as_deref(), Some("Body"));
    }

    #[test]
    fn missing_frontmatter_errors() {
        let raw = "no frontmatter here";
        let err = parse_agent_markdown(
            raw,
            AgentSource::UserDefined,
            PathBuf::from("/tmp"),
            Path::new("x.md"),
        )
        .unwrap_err();
        assert!(matches!(err, AgentLoadError::NoFrontmatter(_)));
    }

    #[test]
    fn frontmatter_tools_become_explicit_policy() {
        let raw = "---\nname: r\ndescription: d\ntools: [Read, Grep]\n---\n";
        let def = parse_agent_markdown(
            raw,
            AgentSource::Project,
            PathBuf::from("/tmp"),
            Path::new("r.md"),
        )
        .unwrap();
        match &def.tools {
            AgentToolPolicy::Explicit(v) => {
                assert_eq!(v, &vec!["Read".to_string(), "Grep".to_string()]);
            }
            other => panic!("expected Explicit, got {other:?}"),
        }
        assert_eq!(
            def.allowed_tools,
            vec!["Read".to_string(), "Grep".to_string()]
        );
    }

    #[tokio::test]
    async fn load_agents_from_dirs_merges_user_and_project() {
        let dir = TempDir::new().unwrap();
        let user = dir.path().join("user");
        let project = dir.path().join("project");
        tokio::fs::create_dir_all(&user).await.unwrap();
        tokio::fs::create_dir_all(&project).await.unwrap();
        tokio::fs::write(
            user.join("alpha.md"),
            "---\nname: alpha\ndescription: from user\n---\n",
        )
        .await
        .unwrap();
        tokio::fs::write(
            project.join("alpha.md"),
            "---\nname: alpha\ndescription: from project\n---\n",
        )
        .await
        .unwrap();
        tokio::fs::write(
            project.join("beta.md"),
            "---\nname: beta\ndescription: project-only\n---\n",
        )
        .await
        .unwrap();

        let defs = load_agents_from_dirs(&[
            (user.clone(), AgentSource::UserDefined),
            (project.clone(), AgentSource::Project),
        ])
        .await;

        assert_eq!(defs.len(), 2);
        // Sorted alphabetically.
        assert_eq!(defs[0].agent_type, "alpha");
        assert_eq!(defs[0].when_to_use, "from project"); // project wins
        assert_eq!(defs[1].agent_type, "beta");
    }

    #[tokio::test]
    async fn load_agents_from_missing_dir_yields_empty() {
        let defs =
            load_agents_from_dirs(&[(PathBuf::from("/does/not/exist"), AgentSource::UserDefined)])
                .await;
        assert!(defs.is_empty());
    }
}
