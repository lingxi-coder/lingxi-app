//! tree-sitter-bash AST helpers for the bash safety layer. Ports the parts of
//! claude-code `src/utils/bash/treeSitterAnalysis.ts` that `bashSecurity.ts`
//! actually consumes. Compiled only under the `bash-ast` feature.

use tree_sitter::{Node, Parser, Tree};

/// Parse `command` as bash. `None` if the parser can't be built or the parse
/// fails outright. A successful-but-error tree still returns `Some`.
fn parse(command: &str) -> Option<Tree> {
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
/// `None` => no tree (parser unavailable) => caller keeps legacy regex behavior.
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
}
