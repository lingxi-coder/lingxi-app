//! Shared MCP server-name normalization used by registry and permission code.

const CLAUDEAI_SERVER_PREFIX: &str = "claude.ai ";

/// Normalize a server name to the token used by `mcp__<server>__<tool>`.
#[must_use]
pub fn normalize_name_for_mcp(name: &str) -> String {
    let mut normalized: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                character
            } else {
                '_'
            }
        })
        .collect();
    if name.starts_with(CLAUDEAI_SERVER_PREFIX) {
        normalized = collapse_and_trim_underscores(&normalized);
    }
    normalized
}

fn collapse_and_trim_underscores(value: &str) -> String {
    let mut collapsed = String::with_capacity(value.len());
    let mut previous_underscore = false;
    for character in value.chars() {
        if character == '_' {
            if !previous_underscore {
                collapsed.push('_');
            }
            previous_underscore = true;
        } else {
            collapsed.push(character);
            previous_underscore = false;
        }
    }
    let trimmed = collapsed
        .strip_prefix('_')
        .unwrap_or(&collapsed)
        .to_string();
    trimmed.strip_suffix('_').unwrap_or(&trimmed).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_ai_names_collapse_delimiter_runs() {
        assert_eq!(normalize_name_for_mcp("claude.ai .a..b "), "claude_ai_a_b");
    }

    #[test]
    fn ordinary_names_preserve_repeated_underscores() {
        assert_eq!(normalize_name_for_mcp("a..b"), "a__b");
    }
}
