//! `/review` — returns the locked code-review prompt as an injected user
//! message so the next turn fetches a GitHub PR and produces a code review.
//!
//! 1:1 behavioral port of the claude-code `review` slash command
//! (`src/commands/review.ts`, a `type: 'prompt'` command whose
//! `getPromptForCommand(args)` returns a single text block built by
//! `LOCAL_REVIEW_PROMPT(args)`). The TS template ends with
//! `PR number: ${args}` — the user-supplied args are always interpolated,
//! whether or not they are empty (matching the TS unconditional `${args}`).
//!
//! Note: `/review` is a builtin but non-core command, so
//! `core_description("review")` resolves to the shared fallback string rather
//! than a dedicated description.

use async_trait::async_trait;
use command_api::builtin_support::names::core_description;
use command_api::model::{BuiltinCommandHandler, CommandResult};
use command_api::parser::ParsedSlashCommand;

/// `/review` handler — returns the code-review prompt as `InjectMessage`.
///
/// Handle-free: `/review` is a static template injection (with the user's args
/// interpolated as the PR number). The next conversation turn picks up the
/// injected content as if the user had typed it.
#[derive(Debug, Default)]
pub struct ReviewHandler;

impl ReviewHandler {
    /// Construct a new `ReviewHandler`.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl BuiltinCommandHandler for ReviewHandler {
    async fn handle(&self, args: &ParsedSlashCommand) -> CommandResult {
        CommandResult::InjectMessage {
            content: build_prompt(&args.raw_args),
        }
    }

    fn name(&self) -> &str {
        "review"
    }

    fn description(&self) -> &str {
        core_description("review")
    }
}

/// Build the code-review prompt, interpolating the user's args as the PR
/// number (mirrors the TS `PR number: ${args}` unconditional interpolation,
/// including the template literal's leading newline and indentation).
fn build_prompt(args: &str) -> String {
    format!(
        "
      You are an expert code reviewer. Follow these steps:

      1. If no PR number is provided in the args, run `gh pr list` to show open PRs
      2. If a PR number is provided, run `gh pr view <number>` to get PR details
      3. Run `gh pr diff <number>` to get the diff
      4. Analyze the changes and provide a thorough code review that includes:
         - Overview of what the PR does
         - Analysis of code quality and style
         - Specific suggestions for improvements
         - Any potential issues or risks

      Keep your review concise but thorough. Focus on:
      - Code correctness
      - Following project conventions
      - Performance implications
      - Test coverage
      - Security considerations

      Format your review with clear sections and bullet points.

      PR number: {args}
    "
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(raw: &str) -> ParsedSlashCommand {
        ParsedSlashCommand {
            name: "review".to_string(),
            raw_args: raw.to_string(),
            positional_args: vec![],
        }
    }

    #[tokio::test]
    async fn returns_inject_message_with_locked_template() {
        let h = ReviewHandler::new();
        match h.handle(&args("")).await {
            CommandResult::InjectMessage { content } => {
                assert!(content.contains("You are an expert code reviewer. Follow these steps:"));
                assert!(content.contains(
                    "1. If no PR number is provided in the args, run `gh pr list` to show open PRs"
                ));
                assert!(
                    content.contains("Format your review with clear sections and bullet points.")
                );
                // Unconditional `${args}` interpolation: empty args => trailing
                // "PR number: " with nothing after it.
                assert!(content.contains("PR number: "));
            }
            other => panic!("expected InjectMessage, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn interpolates_args_as_pr_number() {
        let h = ReviewHandler::new();
        match h.handle(&args("123")).await {
            CommandResult::InjectMessage { content } => {
                assert!(content.contains("PR number: 123"));
            }
            other => panic!("expected InjectMessage, got {other:?}"),
        }
    }

    #[test]
    fn name_and_description() {
        let h = ReviewHandler::new();
        assert_eq!(h.name(), "review");
        assert_eq!(h.description(), core_description("review"));
    }
}
