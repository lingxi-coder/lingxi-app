//! tree-sitter-bash AST helpers for the bash safety layer. Ports the parts of
//! claude-code `src/utils/bash/treeSitterAnalysis.ts` that `bashSecurity.ts`
//! actually consumes. Compiled only under the `bash-ast` feature.

use tree_sitter::{Node, Parser, Tree};

/// Max command length the AST path will parse, mirroring claude-code
/// `utils/bash/parser.ts:19` (`MAX_COMMAND_LENGTH = 10000`). Over-length (and
/// empty) commands return `None` here → the caller keeps the legacy regex path,
/// exactly as `parser.ts:59` returns `null` for `!command || > MAX_COMMAND_LENGTH`.
const MAX_COMMAND_LENGTH: usize = 10_000;

/// Parse `command` as bash. `None` when the command is empty / over the length
/// cap (legacy fallback, claude-code `parser.ts:59`), the parser can't be built,
/// or the parse fails outright. A successful-but-error tree still returns `Some`.
///
/// Acknowledged divergence: claude-code also fail-CLOSES on a parse abort
/// (`PARSE_TIMEOUT_MS=50` / `MAX_NODES=50_000` → `PARSE_ABORTED`). tree-sitter-bash
/// here has no node/time budget; a pathological parse runs to completion and
/// yields a normal tree. Low risk (tree-sitter-bash is robust); a budget could be
/// added later via the pinned parser's timeout API.
fn parse(command: &str) -> Option<Tree> {
    if command.is_empty() || command.len() > MAX_COMMAND_LENGTH {
        return None;
    }
    let mut parser = Parser::new();
    let language: tree_sitter::Language = tree_sitter_bash::LANGUAGE.into();
    parser.set_language(&language).ok()?;
    parser.parse(command, None)
}

/// Iterate ALL children of `node` (named + anonymous operator tokens). One
/// cursor per node (tree-sitter requirement).
fn children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.children(&mut cursor).collect()
}

/// Does the AST contain a real `;`/`&&`/`||`/`list` node? Ports
/// `treeSitterAnalysis.ts` `hasActualOperatorNodes`. `\;` parses as a `word`
/// (not a `;` node), so `find -exec \;` returns `Some(false)`.
/// `None` => no tree (empty / over the `MAX_COMMAND_LENGTH` cap / parser
/// unavailable) => caller keeps the legacy regex behavior.
#[must_use]
pub fn has_actual_operator_nodes(command: &str) -> Option<bool> {
    let tree = parse(command)?;
    Some(walk_has_operator(tree.root_node()))
}

fn walk_has_operator(node: Node) -> bool {
    matches!(node.kind(), ";" | "&&" | "||" | "list")
        || children(node).into_iter().any(walk_has_operator)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_operators_detected() {
        assert_eq!(has_actual_operator_nodes("echo hi ; ls"), Some(true));
        assert_eq!(has_actual_operator_nodes("a && b"), Some(true));
        assert_eq!(has_actual_operator_nodes("a || b"), Some(true));
    }

    #[test]
    fn escaped_semicolon_is_not_an_operator() {
        assert_eq!(has_actual_operator_nodes(r"find . -exec rm {} \;"), Some(false));
        assert_eq!(has_actual_operator_nodes("cat safe.txt \\; echo secret"), Some(false));
    }

    #[test]
    fn plain_command_has_no_operators() {
        assert_eq!(has_actual_operator_nodes("ls -la"), Some(false));
    }

    #[test]
    fn pipeline_without_logical_ops_has_no_list_operators() {
        assert_eq!(has_actual_operator_nodes("cat x | grep y"), Some(false));
    }

    #[test]
    fn empty_and_over_length_return_none() {
        // claude-code parser.ts:59 returns null (legacy fallback) for empty or
        // > MAX_COMMAND_LENGTH commands; we mirror that with None so the caller
        // keeps the legacy regex path (and still flags e.g. a hidden `\;`).
        assert_eq!(has_actual_operator_nodes(""), None);
        let over = format!("echo {} \\; rm -rf /tmp/x", "a".repeat(20_000));
        assert_eq!(has_actual_operator_nodes(&over), None);
        // Exactly at the cap still parses (no operators here).
        assert_eq!(has_actual_operator_nodes(&"a".repeat(10_000)), Some(false));
    }
}
