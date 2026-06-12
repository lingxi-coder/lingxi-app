//! Faithful ~370-line port of claude-code's `BashTool/prompt.ts`
//! `getSimplePrompt()` — the model-facing description of the Bash tool.
//!
//! This is a PROMPT-TEXT parity batch (BASH.6): the goal is byte-faithful
//! reproduction of the EXTERNAL-USER (non-`ant`) prompt that claude-code ships,
//! assembled from the same pieces — header, tool-preference bullets, instruction
//! items, the sandbox section, and the git/PR section.
//!
//! ## Divergences from `prompt.ts` (documented per spec)
//!
//! - **`ant`-only branches dropped.** claude-code's `getSimplePrompt` /
//!   `getCommitAndPRInstructions` have `process.env.USER_TYPE === 'ant'`
//!   branches (undercover instructions, `/commit` + `/commit-push-pr` skill
//!   pointers, the short git section). We are always on the external path, so
//!   those branches are omitted entirely.
//! - **Embedded-search-tools branch dropped.** `hasEmbeddedSearchTools()` gates
//!   whether to steer away from `find`/`grep` (ant-native builds alias them to
//!   bundled bfs/ugrep). External builds always steer toward `Glob`/`Grep`, so
//!   we hardcode the non-embedded path (Glob/Grep bullets present, `find`/`grep`
//!   in the avoid-list, no `find -regex` alternation note).
//! - **Monitor-tool sleep bullets dropped.** The TS `feature('MONITOR_TOOL')`
//!   branch adds Monitor-specific bullets; `Monitor` is a deferred tool here, so
//!   we take the non-Monitor branch verbatim.
//! - **Sandbox section reflects the Rust [`SandboxRuntimeConfig`].** claude-code
//!   drives `getSimpleSandboxSection` off `SandboxManager` getters whose shapes
//!   (`fsReadConfig.allowWithinDeny`, `networkRestrictionConfig.deniedHosts`)
//!   have no analogue on our `SandboxRuntimeConfig`. Those lines are omitted;
//!   see [`sandbox_section`] for the field-by-field mapping. The `$TMPDIR`
//!   cross-user temp-dir normalization (which needs `getClaudeTempDir()`) is not
//!   reproduced — we have no per-UID temp-dir source on the config — so writable
//!   paths are emitted verbatim.
//! - **Tool-name literals are STRING LITERALS** (`"Glob"`, `"Grep"`, `"Read"`,
//!   `"Edit"`, `"Write"`, `"Bash"`) matching the claude-code wire names, rather
//!   than imported constants from other crates (avoids a cross-crate dep).
//! - **Co-Authored-By attribution.** claude-code injects a dynamic
//!   `getAttributionTexts()` commit/PR attribution. This crate has no
//!   attribution source, so the optional attribution clauses are omitted (the
//!   commit step reads "Create the commit with a message." and the example
//!   HEREDOCs carry no trailing attribution) — matching the TS shape when
//!   `commitAttribution`/`prAttribution` are empty.
//!
//! ## BASH.4 note (cwd persistence)
//!
//! The header sentence "The working directory persists between commands, but
//! shell state does not." is kept verbatim because it is what claude-code sends
//! (prompt parity). The actual cwd-persistence BEHAVIOR is a SEPARATE deferred
//! batch (BASH.4): today each `BashTool::call` spawns a fresh shell with
//! `cwd = workspace`, so this sentence is currently ASPIRATIONAL until BASH.4
//! lands the per-session cwd carry-over.

use crate::bash::{BASH_DEFAULT_TIMEOUT_MS, BASH_MAX_TIMEOUT_MS};
use sandbox::runtime_config::SandboxRuntimeConfig;

// ===== Wire tool-name literals (string literals, NOT cross-crate imports) ====

/// `Bash` tool wire name — matches claude-code `BASH_TOOL_NAME`.
const BASH_TOOL_NAME: &str = "Bash";
/// `Glob` tool wire name — matches claude-code `GLOB_TOOL_NAME`.
const GLOB_TOOL_NAME: &str = "Glob";
/// `Grep` tool wire name — matches claude-code `GREP_TOOL_NAME`.
const GREP_TOOL_NAME: &str = "Grep";
/// `Read` tool wire name — matches claude-code `FILE_READ_TOOL_NAME`.
const FILE_READ_TOOL_NAME: &str = "Read";
/// `Edit` tool wire name — matches claude-code `FILE_EDIT_TOOL_NAME`.
const FILE_EDIT_TOOL_NAME: &str = "Edit";
/// `Write` tool wire name — matches claude-code `FILE_WRITE_TOOL_NAME`.
const FILE_WRITE_TOOL_NAME: &str = "Write";

// ===== Helpers ==============================================================

/// A bullet-list node: a top-level item or a group of subitems indented under
/// the preceding top-level item. Mirrors the TS `Array<string | string[]>`.
enum Bullet {
    /// Top-level item, rendered as ` - {item}` (one leading space).
    Item(String),
    /// Subitems, each rendered as `  - {subitem}` (two leading spaces).
    Sub(Vec<String>),
}

/// Port of claude-code `prependBullets` (`constants/prompts.ts:167`):
/// top-level items get ` - `, subitems get `  - `.
fn prepend_bullets(items: &[Bullet]) -> Vec<String> {
    let mut out = Vec::new();
    for item in items {
        match item {
            Bullet::Item(s) => out.push(format!(" - {s}")),
            Bullet::Sub(subs) => {
                for sub in subs {
                    out.push(format!("  - {sub}"));
                }
            }
        }
    }
    out
}

/// Port of `isEnvTruthy` for the one env var this module gates on. claude-code's
/// `isEnvTruthy` treats `"1"`/`"true"` (and any non-empty value other than
/// `"0"`/`"false"`) as truthy; we mirror that for `CLAUDE_CODE_*` toggles.
/// Deliberately divergent from `traits::env::is_env_truthy` (the
/// `envUtils.ts:32-37` allowlist) — denylist semantics, so it stays local.
fn is_env_truthy(name: &str) -> bool {
    match std::env::var(name) {
        Ok(v) => {
            let v = v.trim();
            !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false")
        }
        Err(_) => false,
    }
}

/// Port of `getBackgroundUsageNote` (`prompt.ts:35`). Returns `None` when
/// `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS` is truthy.
fn background_usage_note() -> Option<String> {
    if is_env_truthy("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS") {
        return None;
    }
    Some(
        "You can use the `run_in_background` parameter to run the command in the background. \
         Only use this if you don't need the result immediately and are OK being notified when \
         the command completes later. You do not need to check the output right away - you'll be \
         notified when it finishes. You do not need to use '&' at the end of the command when \
         using this parameter."
            .to_string(),
    )
}

/// Port of `shouldIncludeGitInstructions` (`utils/gitSettings.ts`).
///
/// There is no `gitSettings` analogue in this crate, so this is an always-on
/// stub. TODO(BASH.x): wire to a settings-backed `git.includeGitInstructions`
/// toggle once a settings source lands in tool-shell.
fn should_include_git_instructions() -> bool {
    true
}

// ===== Sandbox section ======================================================

/// Compact `JSON.stringify`-equivalent for the sandbox config objects. serde's
/// default `to_string` is compact (no spaces) — same as `jsonStringify` with no
/// `space` arg.
fn json_compact(value: &serde_json::Value) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

/// Dedup helper mirroring TS `dedup<T>` — preserves first-seen order.
fn dedup(items: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for s in items {
        if seen.insert(s.clone()) {
            out.push(s.clone());
        }
    }
    out
}

/// Port of `getSimpleSandboxSection` (`prompt.ts:172`), driven by the Rust
/// [`SandboxRuntimeConfig`] instead of the TS `SandboxManager` getters.
///
/// Field mapping (TS → Rust; lines with no analogue are OMITTED — see
/// module-doc divergences):
/// - read `denyOnly`           → `filesystem.deny_read`
/// - read `allowWithinDeny`    → (no analogue — omitted)
/// - write `allowOnly`         → `filesystem.allow_write`
/// - write `denyWithinAllow`   → `filesystem.deny_write`
/// - network `allowedHosts`    → `network.allowed_domains`
/// - network `deniedHosts`     → (no analogue — omitted)
/// - `allowUnixSockets`        → `network.allow_unix_sockets`
/// - `ignoreViolations`        → `ignore_violations`
/// - unsandboxed cmds allowed  → `!allow_unsandboxed_commands.is_empty()`
fn sandbox_section(cfg: &SandboxRuntimeConfig) -> String {
    if !cfg.enabled {
        return String::new();
    }

    let allow_unsandboxed_commands = !cfg.allow_unsandboxed_commands.is_empty();

    // Filesystem config object (read.denyOnly + write.allowOnly/denyWithinAllow).
    let filesystem = serde_json::json!({
        "read": {
            "denyOnly": dedup(&cfg.filesystem.deny_read),
        },
        "write": {
            "allowOnly": dedup(&cfg.filesystem.allow_write),
            "denyWithinAllow": dedup(&cfg.filesystem.deny_write),
        },
    });

    // Network config object — only emit keys that have values, mirroring the
    // TS conditional-spread shape (`...(x && { x })`).
    let mut network = serde_json::Map::new();
    if !cfg.network.allowed_domains.is_empty() {
        network.insert(
            "allowedHosts".into(),
            serde_json::json!(dedup(&cfg.network.allowed_domains)),
        );
    }
    if !cfg.network.allow_unix_sockets.is_empty() {
        network.insert(
            "allowUnixSockets".into(),
            serde_json::json!(dedup(&cfg.network.allow_unix_sockets)),
        );
    }

    let mut restriction_lines: Vec<String> = Vec::new();
    restriction_lines.push(format!("Filesystem: {}", json_compact(&filesystem)));
    if !network.is_empty() {
        restriction_lines.push(format!(
            "Network: {}",
            json_compact(&serde_json::Value::Object(network))
        ));
    }
    if !cfg.ignore_violations.is_empty() {
        restriction_lines.push(format!(
            "Ignored violations: {}",
            json_compact(&serde_json::json!(cfg.ignore_violations))
        ));
    }

    let sandbox_override_items: Vec<Bullet> = if allow_unsandboxed_commands {
        vec![
            Bullet::Item("You should always default to running commands within the sandbox. Do NOT attempt to set `dangerouslyDisableSandbox: true` unless:".into()),
            Bullet::Sub(vec![
                "The user *explicitly* asks you to bypass sandbox".into(),
                "A specific command just failed and you see evidence of sandbox restrictions causing the failure. Note that commands can fail for many reasons unrelated to the sandbox (missing files, wrong arguments, network issues, etc.).".into(),
            ]),
            Bullet::Item("Evidence of sandbox-caused failures includes:".into()),
            Bullet::Sub(vec![
                "\"Operation not permitted\" errors for file/network operations".into(),
                "Access denied to specific paths outside allowed directories".into(),
                "Network connection failures to non-whitelisted hosts".into(),
                "Unix socket connection errors".into(),
            ]),
            Bullet::Item("When you see evidence of sandbox-caused failure:".into()),
            Bullet::Sub(vec![
                "Immediately retry with `dangerouslyDisableSandbox: true` (don't ask, just do it)".into(),
                "Briefly explain what sandbox restriction likely caused the failure. Be sure to mention that the user can use the `/sandbox` command to manage restrictions.".into(),
                "This will prompt the user for permission".into(),
            ]),
            Bullet::Item("Treat each command you execute with `dangerouslyDisableSandbox: true` individually. Even if you have recently run a command with this setting, you should default to running future commands within the sandbox.".into()),
            Bullet::Item("Do not suggest adding sensitive paths like ~/.bashrc, ~/.zshrc, ~/.ssh/*, or credential files to the sandbox allowlist.".into()),
        ]
    } else {
        vec![
            Bullet::Item("All commands MUST run in sandbox mode - the `dangerouslyDisableSandbox` parameter is disabled by policy.".into()),
            Bullet::Item("Commands cannot run outside the sandbox under any circumstances.".into()),
            Bullet::Item("If a command fails due to sandbox restrictions, work with the user to adjust sandbox settings instead.".into()),
        ]
    };

    let mut items = sandbox_override_items;
    items.push(Bullet::Item("For temporary files, always use the `$TMPDIR` environment variable. TMPDIR is automatically set to the correct sandbox-writable directory in sandbox mode. Do NOT use `/tmp` directly - use `$TMPDIR` instead.".into()));

    let mut lines: Vec<String> = vec![
        String::new(),
        "## Command sandbox".into(),
        "By default, your command will be run in a sandbox. This sandbox controls which directories and network hosts commands may access or modify without an explicit override.".into(),
        String::new(),
        "The sandbox has the following restrictions:".into(),
        restriction_lines.join("\n"),
        String::new(),
    ];
    lines.extend(prepend_bullets(&items));
    lines.join("\n")
}

// ===== Git / PR section =====================================================

/// Port of `getCommitAndPRInstructions` (`prompt.ts:42`), EXTERNAL-USER branch.
///
/// The `ant` undercover/skills branches are dropped. Returns an empty string
/// when [`should_include_git_instructions`] is false (matching the TS
/// `undercoverSection` early return, which is empty on the external path).
fn commit_and_pr_instructions() -> String {
    if !should_include_git_instructions() {
        return String::new();
    }

    // No attribution source in this crate → commitAttribution / prAttribution
    // are empty, matching the TS shape with empty attribution (commit step says
    // "Create the commit with a message." and example HEREDOCs carry no
    // trailing attribution).
    "# Committing changes with git

Only create commits when requested by the user. If unclear, ask first. When the user asks you to create a new git commit, follow these steps carefully:

You can call multiple tools in a single response. When multiple independent pieces of information are requested and all commands are likely to succeed, run multiple tool calls in parallel for optimal performance. The numbered steps below indicate which commands should be batched in parallel.

Git Safety Protocol:
- NEVER update the git config
- NEVER run destructive git commands (push --force, reset --hard, checkout ., restore ., clean -f, branch -D) unless the user explicitly requests these actions. Taking unauthorized destructive actions is unhelpful and can result in lost work, so it's best to ONLY run these commands when given direct instructions
- NEVER skip hooks (--no-verify, --no-gpg-sign, etc) unless the user explicitly requests it
- NEVER run force push to main/master, warn the user if they request it
- CRITICAL: Always create NEW commits rather than amending, unless the user explicitly requests a git amend. When a pre-commit hook fails, the commit did NOT happen — so --amend would modify the PREVIOUS commit, which may result in destroying work or losing previous changes. Instead, after hook failure, fix the issue, re-stage, and create a NEW commit
- When staging files, prefer adding specific files by name rather than using \"git add -A\" or \"git add .\", which can accidentally include sensitive files (.env, credentials) or large binaries
- NEVER commit changes unless the user explicitly asks you to. It is VERY IMPORTANT to only commit when explicitly asked, otherwise the user will feel that you are being too proactive

1. Run the following bash commands in parallel, each using the Bash tool:
  - Run a git status command to see all untracked files. IMPORTANT: Never use the -uall flag as it can cause memory issues on large repos.
  - Run a git diff command to see both staged and unstaged changes that will be committed.
  - Run a git log command to see recent commit messages, so that you can follow this repository's commit message style.
2. Analyze all staged changes (both previously staged and newly added) and draft a commit message:
  - Summarize the nature of the changes (eg. new feature, enhancement to an existing feature, bug fix, refactoring, test, docs, etc.). Ensure the message accurately reflects the changes and their purpose (i.e. \"add\" means a wholly new feature, \"update\" means an enhancement to an existing feature, \"fix\" means a bug fix, etc.).
  - Do not commit files that likely contain secrets (.env, credentials.json, etc). Warn the user if they specifically request to commit those files
  - Draft a concise (1-2 sentences) commit message that focuses on the \"why\" rather than the \"what\"
  - Ensure it accurately reflects the changes and their purpose
3. Run the following commands in parallel:
   - Add relevant untracked files to the staging area.
   - Create the commit with a message.
   - Run git status after the commit completes to verify success.
   Note: git status depends on the commit completing, so run it sequentially after the commit.
4. If the commit fails due to pre-commit hook: fix the issue and create a NEW commit

Important notes:
- NEVER run additional commands to read or explore code, besides git bash commands
- NEVER use the TodoWrite or Task tools
- DO NOT push to the remote repository unless the user explicitly asks you to do so
- IMPORTANT: Never use git commands with the -i flag (like git rebase -i or git add -i) since they require interactive input which is not supported.
- IMPORTANT: Do not use --no-edit with git rebase commands, as the --no-edit flag is not a valid option for git rebase.
- If there are no changes to commit (i.e., no untracked files and no modifications), do not create an empty commit
- In order to ensure good formatting, ALWAYS pass the commit message via a HEREDOC, a la this example:
<example>
git commit -m \"$(cat <<'EOF'
   Commit message here.
   EOF
   )\"
</example>

# Creating pull requests
Use the gh command via the Bash tool for ALL GitHub-related tasks including working with issues, pull requests, checks, and releases. If given a Github URL use the gh command to get the information needed.

IMPORTANT: When the user asks you to create a pull request, follow these steps carefully:

1. Run the following bash commands in parallel using the Bash tool, in order to understand the current state of the branch since it diverged from the main branch:
   - Run a git status command to see all untracked files (never use -uall flag)
   - Run a git diff command to see both staged and unstaged changes that will be committed
   - Check if the current branch tracks a remote branch and is up to date with the remote, so you know if you need to push to the remote
   - Run a git log command and `git diff [base-branch]...HEAD` to understand the full commit history for the current branch (from the time it diverged from the base branch)
2. Analyze all changes that will be included in the pull request, making sure to look at all relevant commits (NOT just the latest commit, but ALL commits that will be included in the pull request!!!), and draft a pull request title and summary:
   - Keep the PR title short (under 70 characters)
   - Use the description/body for details, not the title
3. Run the following commands in parallel:
   - Create new branch if needed
   - Push to remote with -u flag if needed
   - Create PR using gh pr create with the format below. Use a HEREDOC to pass the body to ensure correct formatting.
<example>
gh pr create --title \"the pr title\" --body \"$(cat <<'EOF'
## Summary
<1-3 bullet points>

## Test plan
[Bulleted markdown checklist of TODOs for testing the pull request...]
EOF
)\"
</example>

Important:
- DO NOT use the TodoWrite or Task tools
- Return the PR URL when you're done, so the user can see it

# Other common operations
- View comments on a Github PR: gh api repos/foo/bar/pulls/123/comments".to_string()
}

// ===== Public entry point ===================================================

/// Port of `getSimplePrompt` (`prompt.ts:275`) — EXTERNAL-USER path.
///
/// `sandbox` drives [`sandbox_section`]; pass the live
/// `BuiltinToolContext::sandbox_runtime`.
#[must_use]
pub fn simple_prompt(sandbox: &SandboxRuntimeConfig) -> String {
    let max_timeout_ms = BASH_MAX_TIMEOUT_MS;
    let default_timeout_ms = BASH_DEFAULT_TIMEOUT_MS;

    let tool_preference_items = vec![
        Bullet::Item(format!("File search: Use {GLOB_TOOL_NAME} (NOT find or ls)")),
        Bullet::Item(format!("Content search: Use {GREP_TOOL_NAME} (NOT grep or rg)")),
        Bullet::Item(format!("Read files: Use {FILE_READ_TOOL_NAME} (NOT cat/head/tail)")),
        Bullet::Item(format!("Edit files: Use {FILE_EDIT_TOOL_NAME} (NOT sed/awk)")),
        Bullet::Item(format!("Write files: Use {FILE_WRITE_TOOL_NAME} (NOT echo >/cat <<EOF)")),
        Bullet::Item("Communication: Output text directly (NOT echo/printf)".into()),
    ];

    // External (non-embedded) avoid-list includes find/grep.
    let avoid_commands = "`find`, `grep`, `cat`, `head`, `tail`, `sed`, `awk`, or `echo`";

    let multiple_commands_subitems = vec![
        format!("If the commands are independent and can run in parallel, make multiple {BASH_TOOL_NAME} tool calls in a single message. Example: if you need to run \"git status\" and \"git diff\", send a single message with two {BASH_TOOL_NAME} tool calls in parallel."),
        format!("If the commands depend on each other and must run sequentially, use a single {BASH_TOOL_NAME} call with '&&' to chain them together."),
        "Use ';' only when you need to run commands sequentially but don't care if earlier commands fail.".into(),
        "DO NOT use newlines to separate commands (newlines are ok in quoted strings).".into(),
    ];

    let git_subitems = vec![
        "Prefer to create a new commit rather than amending an existing commit.".to_string(),
        "Before running destructive operations (e.g., git reset --hard, git push --force, git checkout --), consider whether there is a safer alternative that achieves the same goal. Only use destructive operations when they are truly the best approach.".to_string(),
        "Never skip hooks (--no-verify) or bypass signing (--no-gpg-sign, -c commit.gpgsign=false) unless the user has explicitly asked for it. If a hook fails, investigate and fix the underlying issue.".to_string(),
    ];

    // Non-Monitor branch (Monitor is a deferred tool here).
    let sleep_subitems = vec![
        "Do not sleep between commands that can run immediately — just run them.".to_string(),
        "If your command is long running and you would like to be notified when it finishes — use `run_in_background`. No sleep needed.".to_string(),
        "Do not retry failing commands in a sleep loop — diagnose the root cause.".to_string(),
        "If waiting for a background task you started with `run_in_background`, you will be notified when it completes — do not poll.".to_string(),
        "If you must poll an external process, use a check command (e.g. `gh run view`) rather than sleeping first.".to_string(),
        "If you must sleep, keep the duration short (1-5 seconds) to avoid blocking the user.".to_string(),
    ];

    let background_note = background_usage_note();

    let mut instruction_items: Vec<Bullet> = vec![
        Bullet::Item("If your command will create new directories or files, first use this tool to run `ls` to verify the parent directory exists and is the correct location.".into()),
        Bullet::Item("Always quote file paths that contain spaces with double quotes in your command (e.g., cd \"path with spaces/file.txt\")".into()),
        Bullet::Item("Try to maintain your current working directory throughout the session by using absolute paths and avoiding usage of `cd`. You may use `cd` if the User explicitly requests it.".into()),
        Bullet::Item(format!(
            "You may specify an optional timeout in milliseconds (up to {max_timeout_ms}ms / {} minutes). By default, your command will timeout after {default_timeout_ms}ms ({} minutes).",
            max_timeout_ms / 60_000,
            default_timeout_ms / 60_000
        )),
    ];
    if let Some(note) = background_note {
        instruction_items.push(Bullet::Item(note));
    }
    instruction_items.push(Bullet::Item("When issuing multiple commands:".into()));
    instruction_items.push(Bullet::Sub(multiple_commands_subitems));
    instruction_items.push(Bullet::Item("For git commands:".into()));
    instruction_items.push(Bullet::Sub(git_subitems));
    instruction_items.push(Bullet::Item("Avoid unnecessary `sleep` commands:".into()));
    instruction_items.push(Bullet::Sub(sleep_subitems));

    let mut lines: Vec<String> = vec![
        "Executes a given bash command and returns its output.".into(),
        String::new(),
        // KEPT VERBATIM — prompt parity (see module-doc BASH.4 note: cwd
        // persistence is currently aspirational until BASH.4 lands).
        "The working directory persists between commands, but shell state does not. The shell environment is initialized from the user's profile (bash or zsh).".into(),
        String::new(),
        format!("IMPORTANT: Avoid using this tool to run {avoid_commands} commands, unless explicitly instructed or after you have verified that a dedicated tool cannot accomplish your task. Instead, use the appropriate dedicated tool as this will provide a much better experience for the user:"),
        String::new(),
    ];
    lines.extend(prepend_bullets(&tool_preference_items));
    lines.push(format!("While the {BASH_TOOL_NAME} tool can do similar things, it\u{2019}s better to use the built-in tools as they provide a better user experience and make it easier to review tool calls and give permission."));
    lines.push(String::new());
    lines.push("# Instructions".into());
    lines.extend(prepend_bullets(&instruction_items));
    lines.push(sandbox_section(sandbox));

    let git = commit_and_pr_instructions();
    if !git.is_empty() {
        lines.push(String::new());
        lines.push(git);
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Env vars are process-global; serialize the env-gating tests so they don't
    // race the default-config golden test.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn disabled_sandbox() -> SandboxRuntimeConfig {
        SandboxRuntimeConfig::default()
    }

    #[test]
    fn prompt_contains_locked_anchors() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS");
        let p = simple_prompt(&disabled_sandbox());

        // Header.
        assert!(
            p.contains("Executes a given bash command and returns its output."),
            "missing header anchor"
        );
        // cwd-persistence sentence (kept verbatim for parity).
        assert!(
            p.contains(
                "The working directory persists between commands, but shell state does not."
            ),
            "missing cwd-persistence anchor"
        );
        // run_in_background note (present when env not disabled).
        assert!(
            p.contains("You can use the `run_in_background` parameter to run the command in the background."),
            "missing run_in_background note"
        );
        // Git safety protocol header.
        assert!(
            p.contains("Git Safety Protocol:"),
            "missing git safety protocol header"
        );
        // Committing-changes section header.
        assert!(
            p.contains("# Committing changes with git"),
            "missing committing-changes header"
        );
    }

    #[test]
    fn timeout_sentence_substitutes_locked_constants() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS");
        let p = simple_prompt(&disabled_sandbox());
        // 120000ms / 600000ms substituted from BASH_DEFAULT_TIMEOUT_MS /
        // BASH_MAX_TIMEOUT_MS — and the minute conversions.
        assert_eq!(BASH_MAX_TIMEOUT_MS, 600_000);
        assert_eq!(BASH_DEFAULT_TIMEOUT_MS, 120_000);
        assert!(
            p.contains(
                "You may specify an optional timeout in milliseconds (up to 600000ms / 10 minutes). By default, your command will timeout after 120000ms (2 minutes)."
            ),
            "missing/incorrect timeout sentence; got prompt:\n{p}"
        );
    }

    #[test]
    fn tool_preference_bullets_steer_to_builtin_tools() {
        let _g = ENV_LOCK.lock().unwrap();
        let p = simple_prompt(&disabled_sandbox());
        assert!(p.contains(" - File search: Use Glob (NOT find or ls)"));
        assert!(p.contains(" - Content search: Use Grep (NOT grep or rg)"));
        assert!(p.contains(" - Read files: Use Read (NOT cat/head/tail)"));
        assert!(p.contains(" - Edit files: Use Edit (NOT sed/awk)"));
        assert!(p.contains(" - Write files: Use Write (NOT echo >/cat <<EOF)"));
    }

    #[test]
    fn background_note_absent_when_env_disabled() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS", "1");
        let p = simple_prompt(&disabled_sandbox());
        std::env::remove_var("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS");
        assert!(
            !p.contains("You can use the `run_in_background` parameter"),
            "run_in_background note should be absent when CLAUDE_CODE_DISABLE_BACKGROUND_TASKS=1"
        );
    }

    #[test]
    fn sandbox_section_absent_when_disabled() {
        let _g = ENV_LOCK.lock().unwrap();
        let p = simple_prompt(&disabled_sandbox());
        assert!(
            !p.contains("## Command sandbox"),
            "sandbox section should be absent when sandbox disabled"
        );
    }

    #[test]
    fn sandbox_section_present_when_enabled() {
        let _g = ENV_LOCK.lock().unwrap();
        let cfg = SandboxRuntimeConfig {
            enabled: true,
            filesystem: sandbox::runtime_config::FilesystemRestrictionConfig {
                allow_write: vec!["/work".into()],
                ..Default::default()
            },
            network: sandbox::runtime_config::NetworkRestrictionConfig {
                allowed_domains: vec!["example.com".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        let p = simple_prompt(&cfg);
        assert!(p.contains("## Command sandbox"), "sandbox section missing");
        assert!(
            p.contains("By default, your command will be run in a sandbox."),
            "sandbox intro missing"
        );
        // Filesystem + network restriction lines rendered from the config.
        assert!(p.contains("Filesystem: "), "filesystem line missing");
        assert!(
            p.contains("\"allowOnly\":[\"/work\"]"),
            "writable path not inlined; got:\n{p}"
        );
        assert!(
            p.contains("Network: ") && p.contains("\"allowedHosts\":[\"example.com\"]"),
            "network line missing; got:\n{p}"
        );
        // disabled-by-policy branch (allow_unsandboxed_commands empty).
        assert!(
            p.contains("All commands MUST run in sandbox mode"),
            "policy-disabled override branch missing"
        );
        // $TMPDIR bullet always present.
        assert!(p.contains("`$TMPDIR` environment variable"));
    }

    #[test]
    fn sandbox_section_unsandboxed_allowed_branch() {
        let _g = ENV_LOCK.lock().unwrap();
        let cfg = SandboxRuntimeConfig {
            enabled: true,
            allow_unsandboxed_commands: vec!["bazel".into()],
            ..Default::default()
        };
        let p = simple_prompt(&cfg);
        assert!(
            p.contains("You should always default to running commands within the sandbox."),
            "allow-unsandboxed override branch missing"
        );
        assert!(
            !p.contains("All commands MUST run in sandbox mode"),
            "policy-disabled branch should not appear when unsandboxed cmds allowed"
        );
    }

    #[test]
    fn prepend_bullets_indentation() {
        let items = vec![
            Bullet::Item("top".into()),
            Bullet::Sub(vec!["sub".into()]),
        ];
        let out = prepend_bullets(&items);
        assert_eq!(out, vec![" - top".to_string(), "  - sub".to_string()]);
    }
}
