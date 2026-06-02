//! `/pr-comments` — returns the locked PR-comments prompt as an injected user
//! message so the next turn fetches and formats comments from a GitHub PR.
//!
//! 1:1 behavioral port of the claude-code `pr-comments` slash command
//! (`src/commands/pr_comments/index.ts`, a `createMovedToPluginCommand` whose
//! `getPromptWhileMarketplaceIsPrivate(args)` returns the prompt text below).
//! The TS template ends with `${args ? 'Additional user input: ' + args : ''}`,
//! so user-supplied args are appended only when non-empty.

use async_trait::async_trait;
use command_api::builtin_support::names::core_description;
use command_api::model::{BuiltinCommandHandler, CommandResult};
use command_api::parser::ParsedSlashCommand;

/// `/pr-comments` handler — returns the PR-comments prompt as `InjectMessage`.
///
/// Handle-free: `/pr-comments` is a static template injection (with optional
/// trailing user input). The next conversation turn picks up the injected
/// content as if the user had typed it.
#[derive(Debug, Default)]
pub struct PrCommentsHandler;

impl PrCommentsHandler {
    /// Construct a new `PrCommentsHandler`.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl BuiltinCommandHandler for PrCommentsHandler {
    async fn handle(&self, args: &ParsedSlashCommand) -> CommandResult {
        CommandResult::InjectMessage {
            content: build_prompt(&args.raw_args),
        }
    }

    fn name(&self) -> &str {
        "pr-comments"
    }

    fn description(&self) -> &str {
        core_description("pr-comments")
    }
}

/// Build the PR-comments prompt, appending `Additional user input:` only when
/// `args` is non-empty (mirrors the TS `${args ? ... : ''}` interpolation).
fn build_prompt(args: &str) -> String {
    let additional = if args.is_empty() {
        String::new()
    } else {
        format!("Additional user input: {args}")
    };
    format!(
        r#"You are an AI assistant integrated into a git-based version control system. Your task is to fetch and display comments from a GitHub pull request.

Follow these steps:

1. Use `gh pr view --json number,headRepository` to get the PR number and repository info
2. Use `gh api /repos/{{owner}}/{{repo}}/issues/{{number}}/comments` to get PR-level comments
3. Use `gh api /repos/{{owner}}/{{repo}}/pulls/{{number}}/comments` to get review comments. Pay particular attention to the following fields: `body`, `diff_hunk`, `path`, `line`, etc. If the comment references some code, consider fetching it using eg `gh api /repos/{{owner}}/{{repo}}/contents/{{path}}?ref={{branch}} | jq .content -r | base64 -d`
4. Parse and format all comments in a readable way
5. Return ONLY the formatted comments, with no additional text

Format the comments as:

## Comments

[For each comment thread:]
- @author file.ts#line:
  ```diff
  [diff_hunk from the API response]
  ```
  > quoted comment text

  [any replies indented]

If there are no comments, return "No comments found."

Remember:
1. Only show the actual comments, no explanatory text
2. Include both PR-level and code review comments
3. Preserve the threading/nesting of comment replies
4. Show the file and line number context for code review comments
5. Use jq to parse the JSON responses from the GitHub API

{additional}
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(raw: &str) -> ParsedSlashCommand {
        ParsedSlashCommand {
            name: "pr-comments".to_string(),
            raw_args: raw.to_string(),
            positional_args: vec![],
        }
    }

    #[tokio::test]
    async fn returns_inject_message_with_locked_template() {
        let h = PrCommentsHandler::new();
        match h.handle(&args("")).await {
            CommandResult::InjectMessage { content } => {
                assert!(content.contains(
                    "You are an AI assistant integrated into a git-based version control system."
                ));
                assert!(content.contains("If there are no comments, return \"No comments found.\""));
                // No args => no "Additional user input:" line.
                assert!(!content.contains("Additional user input:"));
            }
            other => panic!("expected InjectMessage, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn appends_additional_user_input_when_args_present() {
        let h = PrCommentsHandler::new();
        match h.handle(&args("focus on the auth changes")).await {
            CommandResult::InjectMessage { content } => {
                assert!(content.contains("Additional user input: focus on the auth changes"));
            }
            other => panic!("expected InjectMessage, got {other:?}"),
        }
    }

    #[test]
    fn name_and_description() {
        let h = PrCommentsHandler::new();
        assert_eq!(h.name(), "pr-comments");
        assert_eq!(h.description(), core_description("pr-comments"));
    }
}
