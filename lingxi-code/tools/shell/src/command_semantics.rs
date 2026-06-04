//! Exit-code reinterpretation — 1:1 port of claude-code
//! `src/tools/BashTool/commandSemantics.ts` (`interpretCommandResult`).
//!
//! Many commands use the exit code to convey information other than
//! success/failure: `grep`/`rg` return 1 for "no matches", `find` returns 1
//! for "some directories inaccessible", `diff` returns 1 for "differences
//! found", `test`/`[` return 1 for "condition false". Treating those as errors
//! (the naive `exit_code != 0`) wrongly reports them to the model as failures.
//! This maps the per-command semantics so exit 1 on those commands is a
//! NON-error with a descriptive note, while exit ≥ 2 stays an error.

use permission::shell_command::split_command;

/// Outcome of [`interpret_command_result`].
pub struct CommandInterpretation {
    /// Whether the model should treat the result as an error.
    pub is_error: bool,
    /// Optional descriptive note (claude-code `returnCodeInterpretation`),
    /// `None` when the default semantic produced no message.
    pub message: Option<String>,
}

/// Interpret a finished command's exit code with command-specific semantics
/// (claude-code `interpretCommandResult`). `stdout`/`stderr` are unused by the
/// ported semantics (they are part of the TS signature but never consulted).
#[must_use]
pub fn interpret_command_result(command: &str, exit_code: i32) -> CommandInterpretation {
    match base_command(command).as_str() {
        // grep / ripgrep: 0=matches, 1=no matches, 2+=error.
        "grep" | "rg" => CommandInterpretation {
            is_error: exit_code >= 2,
            message: (exit_code == 1).then(|| "No matches found".to_string()),
        },
        // find: 0=success, 1=partial (some dirs inaccessible), 2+=error.
        "find" => CommandInterpretation {
            is_error: exit_code >= 2,
            message: (exit_code == 1).then(|| "Some directories were inaccessible".to_string()),
        },
        // diff: 0=no differences, 1=differences found, 2+=error.
        "diff" => CommandInterpretation {
            is_error: exit_code >= 2,
            message: (exit_code == 1).then(|| "Files differ".to_string()),
        },
        // test / [: 0=condition true, 1=condition false, 2+=error.
        "test" | "[" => CommandInterpretation {
            is_error: exit_code >= 2,
            message: (exit_code == 1).then(|| "Condition is false".to_string()),
        },
        // Everything else: default semantic (any non-zero is an error).
        _ => CommandInterpretation {
            is_error: exit_code != 0,
            message: (exit_code != 0).then(|| format!("Command failed with exit code {exit_code}")),
        },
    }
}

/// `heuristicallyExtractBaseCommand`: the exit code is determined by the LAST
/// subcommand, so split into subcommands (the `splitCommand_DEPRECATED`
/// analogue) and take the first whitespace token of the last segment. "May get
/// it wrong — not used for security" (claude-code comment).
fn base_command(command: &str) -> String {
    let segments = split_command(command);
    let last = segments
        .last()
        .map_or(command, std::string::String::as_str);
    extract_base_command(last)
}

/// First whitespace-delimited token of a single command (`extractBaseCommand`).
fn extract_base_command(command: &str) -> String {
    command.split_whitespace().next().unwrap_or("").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grep_no_match_is_not_an_error() {
        let r = interpret_command_result("grep needle file.txt", 1);
        assert!(!r.is_error);
        assert_eq!(r.message.as_deref(), Some("No matches found"));
    }

    #[test]
    fn grep_real_error_is_error() {
        let r = interpret_command_result("grep -X bad", 2);
        assert!(r.is_error);
        assert_eq!(r.message, None);
    }

    #[test]
    fn rg_find_diff_test_messages() {
        assert_eq!(
            interpret_command_result("rg pat", 1).message.as_deref(),
            Some("No matches found")
        );
        assert_eq!(
            interpret_command_result("find . -name x", 1).message.as_deref(),
            Some("Some directories were inaccessible")
        );
        assert_eq!(
            interpret_command_result("diff a b", 1).message.as_deref(),
            Some("Files differ")
        );
        assert_eq!(
            interpret_command_result("test -f x", 1).message.as_deref(),
            Some("Condition is false")
        );
        assert_eq!(
            interpret_command_result("[ -f x ]", 1).message.as_deref(),
            Some("Condition is false")
        );
    }

    #[test]
    fn last_subcommand_determines_semantic() {
        // The pipeline's exit code is the LAST command's: `... | grep x`.
        let r = interpret_command_result("cat f | grep needle", 1);
        assert!(!r.is_error);
        assert_eq!(r.message.as_deref(), Some("No matches found"));
        // `grep x && echo done` → last is `echo`, default semantic.
        let r2 = interpret_command_result("grep x f && echo done", 1);
        assert!(r2.is_error);
        assert_eq!(r2.message.as_deref(), Some("Command failed with exit code 1"));
    }

    #[test]
    fn default_semantic_for_unmapped_commands() {
        let ok = interpret_command_result("echo hi", 0);
        assert!(!ok.is_error);
        assert_eq!(ok.message, None);
        let bad = interpret_command_result("cargo build", 101);
        assert!(bad.is_error);
        assert_eq!(bad.message.as_deref(), Some("Command failed with exit code 101"));
    }

    #[test]
    fn zero_exit_never_errors() {
        assert!(!interpret_command_result("grep x f", 0).is_error);
        assert_eq!(interpret_command_result("grep x f", 0).message, None);
    }
}
