//! `TeammateViewHeader` (claude-code `TeammateViewHeader.tsx`):
//! `Viewing @{name} · esc to return` then a second line with the prompt.

/// ` · esc to return` (space + U+00B7 + space + text).
const RETURN_HINT: &str = " \u{00B7} esc to return";

/// Two-line header: `Viewing @{name} · esc to return` then `{prompt}`.
/// An empty prompt omits the second line.
#[must_use]
pub fn render_teammate_view_header(name: &str, prompt: &str) -> String {
    let head = format!("Viewing @{name}{RETURN_HINT}");
    if prompt.is_empty() {
        head
    } else {
        format!("{head}\n{prompt}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_prompt() {
        assert_eq!(
            render_teammate_view_header("alice", "Refactor the parser"),
            "Viewing @alice \u{00B7} esc to return\nRefactor the parser"
        );
    }

    #[test]
    fn without_prompt() {
        assert_eq!(
            render_teammate_view_header("bob", ""),
            "Viewing @bob \u{00B7} esc to return"
        );
    }
}
