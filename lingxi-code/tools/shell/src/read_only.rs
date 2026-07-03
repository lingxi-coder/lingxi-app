//! Read-only / concurrency classification for bash commands — a faithful port
//! of the CORE of claude-code
//! `src/tools/BashTool/readOnlyValidation.ts::checkReadOnlyConstraints`
//! (+ `isCommandReadOnly` / the `READONLY_COMMANDS` allowlist / the hand-written
//! `READONLY_COMMAND_REGEXES`) and `bashPermissions.ts::commandHasAnyCd`.
//!
//! A command is read-only iff EVERY subcommand (split on `&&`/`||`/`;`/`|`/
//! newline via [`permission::shell_command::split_command`], the analogue of
//! claude-code `splitCommand_DEPRECATED`) has a base command in the read-only
//! allowlist AND carries no unsafe construct (output redirection, command
//! substitution, brace/glob expansion that could smuggle a dangerous flag,
//! `cd`, or a per-command dangerous flag like `find -exec`).
//!
//! Consumed by [`crate::bash::BashTool::is_read_only`] /
//! `is_concurrency_safe` (both delegate to `check_read_only`, exactly as
//! claude-code `BashTool.tsx:434-441` delegates to `checkReadOnlyConstraints` +
//! `commandHasAnyCd`). The live `PolicyPermissionGate` then auto-allows a
//! read-only command without a prompt, and the orchestrator may schedule it
//! concurrently.
//!
//! ## Deferred (documented divergences vs. claude-code)
//! `readOnlyValidation.ts` is ~2000 lines; this batch is scoped to the
//! conservative core. The following are DEFERRED — each makes us classify
//! *fewer* commands as read-only (an extra permission prompt), so each is a
//! safe-direction divergence that can NEVER produce an over-allow:
//!
//! - **Tree-sitter AST path.** claude-code tokenizes via `shell-quote` /
//!   tree-sitter (`tryParseShellCommand`); we use the quote-aware delimiter
//!   splitter (`split_command`, the documented `splitCommand_DEPRECATED`
//!   analogue) plus a quote-aware token scan. A handful of adversarial
//!   single-quote/backslash inputs that the TS AST normalizes fall through to
//!   "not read-only" here.
//! - **Full `COMMAND_ALLOWLIST` + `validateFlags`.** claude-code has a
//!   declarative per-flag allowlist (`isCommandSafeViaFlagParsing`) covering
//!   `xargs`, `file`, `sort`, `jq`, `fd`, the full `git`/`gh`/`docker`/`rg`/
//!   `pyright` read-only flag matrices, etc. We port the simpler, robust
//!   `READONLY_COMMANDS` base-name allowlist + the hand-written regex commands
//!   (`ls`, `find`, `echo`, `pwd`, `whoami`, `uniq`, version checks, …) + a
//!   conservative `git`/`gh` read-only subcommand recognizer. Commands that
//!   would be auto-allowed only via the flag-parser (e.g. `sort -k2`, `jq '.x'`)
//!   fall through to "not read-only" — a prompt, not a bypass.
//! - **Windows UNC-path / bare-git-repo / git-internal-write checks** and the
//!   sandbox-cwd race guard (`containsVulnerableUncPath`,
//!   `isCurrentDirectoryBareGitRepo`, `commandWritesToGitInternalPaths`,
//!   `getCwd() !== getOriginalCwd()`): these only ever turn an "allow" into a
//!   "passthrough" in TS. Omitting the *allow path* for those exotic shapes is
//!   already covered because we conservatively reject globs/`$`/substitution.
//!   The `compound cd + git` guard IS ported (the `cd`-present check rejects it).
//! - **`ANT_ONLY_COMMAND_ALLOWLIST`** (ant-internal commands) — out of scope.
//!
//! ## Conservative bias
//! When in doubt we return [`ReadOnlyBehavior::Passthrough`] (not read-only),
//! matching claude-code's `behavior: 'passthrough'` fallthrough — an extra
//! prompt, never an over-allow.

use permission::shell_command::split_command;

/// Verdict mirroring the `behavior` field of claude-code's `PermissionResult`
/// as produced by `checkReadOnlyConstraints`. Only the `allow`/`passthrough`
/// distinction matters for the read-only classification (`isReadOnly` checks
/// `result.behavior === 'allow'`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadOnlyBehavior {
    /// Every subcommand is read-only → the gate may auto-allow without a prompt.
    Allow,
    /// Not provably read-only → fall through to the normal permission checks.
    Passthrough,
}

/// Result of [`check_read_only`]. Carries the verdict plus an optional human
/// message (the locked passthrough strings from `checkReadOnlyConstraints`),
/// mirroring `PermissionResult { behavior, message }`.
#[derive(Debug, Clone)]
pub struct ReadOnlyResult {
    /// The classification verdict.
    pub behavior: ReadOnlyBehavior,
    /// Optional explanation, present on the passthrough paths.
    pub message: Option<String>,
}

impl ReadOnlyResult {
    /// Convenience: is this command classified read-only?
    #[must_use]
    pub fn is_read_only(&self) -> bool {
        self.behavior == ReadOnlyBehavior::Allow
    }

    fn allow() -> Self {
        Self {
            behavior: ReadOnlyBehavior::Allow,
            message: None,
        }
    }

    fn passthrough(message: &str) -> Self {
        Self {
            behavior: ReadOnlyBehavior::Passthrough,
            message: Some(message.to_string()),
        }
    }
}

/// Simple read-only base commands. Port of `READONLY_COMMANDS`
/// (`readOnlyValidation.ts:1432-1503`) plus `EXTERNAL_READONLY_COMMANDS`
/// single-word entries. Each must NOT have flags that write files, execute
/// code, or make network requests — validated against the safe-argument
/// character set by [`has_only_safe_args`] (the `makeRegexForSafeCommand`
/// analogue). Multi-token external commands (`docker ps`, `docker images`) are
/// handled by [`is_external_readonly_prefix`].
const READONLY_COMMANDS: &[&str] = &[
    // Time and date
    "cal", "uptime", "date", // File content viewing
    "cat", "head", "tail", "wc", "stat", "strings", "hexdump", "od", "nl", // System info
    "id", "uname", "free", "df", "du", "locale", "groups", "nproc",
    // Path information
    "basename", "dirname", "realpath", "readlink", // Text processing
    "cut", "paste", "tr", "column", "tac", "rev", "fold", "expand", "unexpand", "fmt", "comm",
    "cmp", "numfmt", // File comparison
    "diff",   // true / false
    "true", "false", // Misc. safe commands
    "sleep", "which", "type", "expr", "test", "getconf", "seq", "tsort", "pr",
    // Read-only search (no dangerous flags in the simple invocation; the
    // dangerous-flag guards below reject the unsafe ones).
    "grep", "egrep", "fgrep", "rg", "sort",
];

/// Exact-match read-only commands taking no (or only enumerated) arguments.
/// Port of the bare-command `READONLY_COMMAND_REGEXES`
/// (`readOnlyValidation.ts:1528-1546`).
const READONLY_EXACT: &[&str] = &[
    "pwd",
    "whoami",
    "alias",
    "ip addr",
    "claude -h",
    "claude --help",
    "node -v",
    "node --version",
    "python --version",
    "python3 --version",
];

/// Multi-token external read-only command prefixes — port of the multi-word
/// entries of `EXTERNAL_READONLY_COMMANDS` (`docker ps`, `docker images`).
const EXTERNAL_READONLY_PREFIXES: &[&str] = &["docker ps", "docker images"];

/// Conservative `git`/`gh` read-only subcommand recognizer. Port of the KEYS of
/// `GIT_READ_ONLY_COMMANDS` / `GH_READ_ONLY_COMMANDS`
/// (`readOnlyCommandValidation.ts`). We accept these multi-word prefixes as
/// read-only (subject to the global `git -c` / `--exec-path` / `--config-env`
/// guards) but DO NOT port the per-flag allowlist — extra flags still pass the
/// safe-argument character scan, which already rejects redirection /
/// substitution / globs.
const GIT_READONLY_PREFIXES: &[&str] = &[
    "git diff",
    "git log",
    "git show",
    "git shortlog",
    "git reflog",
    "git stash list",
    "git stash show",
    "git status",
    "git blame",
    "git ls-files",
    "git remote show",
    "git remote",
    "git merge-base",
    "git rev-parse",
    "git rev-list",
    "git describe",
    "git cat-file",
    "git for-each-ref",
    "git grep",
    "git worktree list",
    "git tag",
    "git branch",
];

const GH_READONLY_PREFIXES: &[&str] = &[
    "gh pr view",
    "gh pr list",
    "gh pr diff",
    "gh pr checks",
    "gh pr status",
    "gh issue view",
    "gh issue list",
    "gh issue status",
    "gh repo view",
    "gh run list",
    "gh run view",
    "gh auth status",
    "gh release list",
    "gh release view",
    "gh workflow list",
    "gh workflow view",
    "gh label list",
];

/// `find` flags that write files, execute code, or otherwise escape read-only.
/// Port of the negative-lookahead set in the `find` regex
/// (`readOnlyValidation.ts:1569`).
const FIND_DANGEROUS_FLAGS: &[&str] = &[
    "-delete", "-exec", "-execdir", "-ok", "-okdir", "-fprint", "-fprint0", "-fls", "-fprintf",
];

/// Top-level entry point. Port of `checkReadOnlyConstraints`
/// (`readOnlyValidation.ts:1876`).
///
/// `compound_has_cd` is the pre-computed [`command_has_any_cd`] flag (passed in
/// to mirror the TS signature, which threads it from `BashTool.tsx:438`).
///
/// Returns [`ReadOnlyBehavior::Allow`] iff every subcommand is provably
/// read-only and no `cd`/`pushd`/`popd` is present anywhere in the compound
/// command; otherwise [`ReadOnlyBehavior::Passthrough`].
#[must_use]
pub fn check_read_only(command: &str, compound_has_cd: bool) -> ReadOnlyResult {
    let command = command.trim();
    if command.is_empty() {
        return ReadOnlyResult::passthrough(
            "Command cannot be parsed, requires further permission checks",
        );
    }

    // SECURITY (port of the `compoundCommandHasCd && hasGitCommand` guard plus
    // the broader posture): a `cd`/`pushd`/`popd` anywhere makes the command
    // non-read-only. claude-code's narrower guard only blocks `cd + git`, but a
    // bare `cd` is itself NOT in our read-only allowlist (it changes shell
    // state), so any compound with `cd` is already non-read-only. Rejecting it
    // up-front is STRICTER than TS (TS would still allow `cd /tmp && ls` if it
    // were not for the cd-not-being-read-only fact) and never an over-allow.
    if compound_has_cd {
        return ReadOnlyResult::passthrough(
            "Command is not read-only, requires further permission checks",
        );
    }

    let subcommands = split_command(command);
    if subcommands.is_empty() {
        return ReadOnlyResult::passthrough(
            "Command is not read-only, requires further permission checks",
        );
    }

    let all_read_only = subcommands.iter().all(|sub| is_command_read_only(sub));
    if all_read_only {
        ReadOnlyResult::allow()
    } else {
        ReadOnlyResult::passthrough("Command is not read-only, requires further permission checks")
    }
}

/// Does a compound command contain ANY `cd`/`pushd`/`popd` subcommand?
/// Port of `commandHasAnyCd` (`bashPermissions.ts:2617`) + `isNormalizedCdCommand`
/// (`:2603`). Note this is intentionally a BASE-COMMAND check, not a substring
/// scan: `echo cd` is NOT a cd (the base command is `echo`).
#[must_use]
pub fn command_has_any_cd(command: &str) -> bool {
    split_command(command)
        .iter()
        .any(|sub| is_normalized_cd_command(sub))
}

/// Port of `isNormalizedCdCommand` (`bashPermissions.ts:2603`): the base
/// command (after stripping leading `KEY=val` env prefixes) is `cd`, `pushd`,
/// or `popd`.
fn is_normalized_cd_command(subcommand: &str) -> bool {
    let stripped = strip_leading_env_vars(subcommand.trim());
    matches!(
        base_command(stripped).as_deref(),
        Some("cd" | "pushd" | "popd")
    )
}

/// Port of `isCommandReadOnly` (`readOnlyValidation.ts:1678`): is a SINGLE
/// subcommand read-only?
fn is_command_read_only(subcommand: &str) -> bool {
    // Handle the common `cmd 2>&1` stderr-to-stdout redirection (the only
    // redirection treated as benign — port of `readOnlyValidation.ts:1683`).
    let mut test_command = subcommand.trim().to_string();
    if let Some(stripped) = test_command.strip_suffix(" 2>&1") {
        test_command = stripped.trim().to_string();
    }
    if test_command.is_empty() {
        return false;
    }

    // Reject command substitution / unescaped backticks / process substitution.
    // (Defense-in-depth analogue of `bashCommandIsSafe_DEPRECATED` +
    // `containsUnquotedExpansion` — we cannot know what these expand to.)
    if contains_dangerous_construct(&test_command) {
        return false;
    }

    // Reject any output redirection other than the `2>&1` handled above
    // (port of the implicit "no redirect" posture — a `>`/`>>`/`<` etc. write
    // or read escapes read-only).
    if contains_output_redirection(&test_command) {
        return false;
    }

    // Reject unquoted glob (`*?[]`) or expandable `$` (port of
    // `containsUnquotedExpansion`, `readOnlyValidation.ts:1600`).
    if contains_unquoted_expansion(&test_command) {
        return false;
    }

    let Some(base) = base_command(&test_command) else {
        return false;
    };

    // git / gh read-only subcommand handling (with the global git config guards).
    if base == "git" {
        return is_git_read_only(&test_command);
    }
    if base == "gh" {
        return matches_multiword_prefix(&test_command, GH_READONLY_PREFIXES);
    }

    // Multi-word external read-only commands (`docker ps`, `docker images`).
    if is_external_readonly_prefix(&test_command) {
        return true;
    }

    // `find` — read-only only when no dangerous flag is present
    // (port of the `find` regex negative-lookahead, `readOnlyValidation.ts:1569`).
    if base == "find" {
        return find_is_read_only(&test_command);
    }

    // `ls` — read-only with safe argument characters
    // (port of `/^ls(?:\s+[^<>()$`|{}&;\n\r]*)?$/`).
    if base == "ls" {
        return has_only_safe_ls_args(&test_command);
    }

    // `echo` — port of the echo regex (`readOnlyValidation.ts:1516`): no
    // command-substitution / pipe / redirect metacharacters.
    if base == "echo" {
        return echo_is_read_only(&test_command);
    }

    // Exact-match read-only commands (pwd, whoami, version checks, …).
    if READONLY_EXACT.iter().any(|&c| c == test_command) {
        return true;
    }

    // Simple read-only commands with a safe argument character set
    // (port of `makeRegexForSafeCommand`).
    if READONLY_COMMANDS.contains(&base.as_str()) {
        return has_only_safe_args(&test_command);
    }

    false
}

/// Port of the git config guards + read-only subcommand check from
/// `isCommandReadOnly` (`readOnlyValidation.ts:1726-1748`) and the
/// `GIT_READ_ONLY_COMMANDS` keys. `git ls-remote` is intentionally NOT included
/// (its URL/exfiltration handling is in the deferred flag-parser).
fn is_git_read_only(command: &str) -> bool {
    // Block `git -c` (inline config → code exec), `--exec-path` (path
    // manipulation), `--config-env` (config via env). Match the flag preceded by
    // whitespace and followed by whitespace or `=` (port of `\s-c[\s=]` etc.).
    if has_flag_with_value_boundary(command, "-c")
        || has_flag_with_value_boundary(command, "--exec-path")
        || has_flag_with_value_boundary(command, "--config-env")
    {
        return false;
    }
    matches_multiword_prefix(command, GIT_READONLY_PREFIXES)
}

/// Does `command` contain `<flag>` preceded by whitespace and followed by
/// whitespace or `=`? Port of the `/\s-c[\s=]/` family
/// (`readOnlyValidation.ts:1726`).
fn has_flag_with_value_boundary(command: &str, flag: &str) -> bool {
    let bytes: Vec<char> = command.chars().collect();
    let flag_chars: Vec<char> = flag.chars().collect();
    let n = flag_chars.len();
    for start in 1..bytes.len() {
        // Preceding char must be whitespace.
        if !bytes[start - 1].is_whitespace() {
            continue;
        }
        if start + n > bytes.len() {
            continue;
        }
        if bytes[start..start + n] != flag_chars[..] {
            continue;
        }
        // Following char must be whitespace or `=`.
        match bytes.get(start + n) {
            Some(c) if c.is_whitespace() || *c == '=' => return true,
            _ => {}
        }
    }
    false
}

/// Does the command's base + following tokens form one of the multi-word
/// read-only prefixes? Mirrors the multi-word match loop in
/// `isCommandSafeViaFlagParsing` (`readOnlyValidation.ts:1284-1300`).
fn matches_multiword_prefix(command: &str, prefixes: &[&str]) -> bool {
    let tokens: Vec<&str> = command.split_whitespace().collect();
    for &prefix in prefixes {
        let prefix_tokens: Vec<&str> = prefix.split(' ').collect();
        if tokens.len() >= prefix_tokens.len() && tokens[..prefix_tokens.len()] == prefix_tokens[..]
        {
            return true;
        }
    }
    false
}

/// Port of `is in EXTERNAL_READONLY_COMMANDS` for the multi-word entries.
fn is_external_readonly_prefix(command: &str) -> bool {
    matches_multiword_prefix(command, EXTERNAL_READONLY_PREFIXES)
}

/// `find` read-only check: base is `find`, no dangerous flag token present.
fn find_is_read_only(command: &str) -> bool {
    let tokens: Vec<&str> = command.split_whitespace().collect();
    for tok in &tokens[1..] {
        // Block dangerous flags (exact or `-flag=...`).
        if FIND_DANGEROUS_FLAGS
            .iter()
            .any(|&f| *tok == f || tok.starts_with(&format!("{f}=")))
        {
            return false;
        }
    }
    true
}

/// `echo` read-only: port of the echo regex (`readOnlyValidation.ts:1516`).
/// No command-substitution / pipe / redirect / brace / paren metacharacters
/// outside single quotes.
fn echo_is_read_only(command: &str) -> bool {
    // Re-uses the dangerous-construct + redirection + expansion guards already
    // applied by the caller; additionally disallow the metacharacters the echo
    // regex excludes that the generic guards might not (`{` `}` `(` `)` `!`).
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    for ch in command.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && !in_single {
            escaped = true;
            continue;
        }
        if ch == '\'' && !in_double {
            in_single = !in_single;
            continue;
        }
        if ch == '"' && !in_single {
            in_double = !in_double;
            continue;
        }
        if in_single {
            continue;
        }
        if matches!(ch, '{' | '}' | '(' | ')' | '#' | '!') {
            return false;
        }
    }
    true
}

/// `ls` argument check: port of `/^ls(?:\s+[^<>()$`|{}&;\n\r]*)?$/`. The generic
/// guards already reject `$`/glob/redirection; here we additionally reject the
/// remaining excluded metacharacters outside quotes.
fn has_only_safe_ls_args(command: &str) -> bool {
    has_only_chars_outside_quotes(command, &['<', '>', '(', ')', '`', '|', '{', '}', '&', ';'])
}

/// Generic safe-argument check for `READONLY_COMMANDS`. Port of
/// `makeRegexForSafeCommand`: after the command name, only characters NOT in
/// `[<>()$`|{}&;\n\r]` are allowed (the `$` / glob cases are handled by the
/// expansion guard for stronger coverage).
fn has_only_safe_args(command: &str) -> bool {
    has_only_chars_outside_quotes(command, &['<', '>', '(', ')', '`', '|', '{', '}', '&', ';'])
}

/// Returns true if NONE of the `forbidden` characters appear OUTSIDE single
/// quotes (single-quoted text is literal in bash, so metacharacters there are
/// inert — matching `makeRegexForSafeCommand`'s allowance of `'...'`).
fn has_only_chars_outside_quotes(command: &str, forbidden: &[char]) -> bool {
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    for ch in command.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && !in_single {
            escaped = true;
            continue;
        }
        if ch == '\'' && !in_double {
            in_single = !in_single;
            continue;
        }
        if ch == '"' && !in_single {
            in_double = !in_double;
            continue;
        }
        if ch == '\n' || ch == '\r' {
            return false;
        }
        // `makeRegexForSafeCommand` allows these chars only inside single
        // quotes (literal). Double-quoted text can still expand `$`/backtick,
        // so we only treat single-quoted text as inert here.
        if in_single {
            continue;
        }
        if forbidden.contains(&ch) {
            return false;
        }
    }
    !in_single && !in_double
}

/// Reject command substitution (`$(`, backticks) and process substitution
/// (`<(`, `>(`). Conservative analogue of the substitution checks in
/// `bashCommandIsSafe_DEPRECATED`.
fn contains_dangerous_construct(command: &str) -> bool {
    let chars: Vec<char> = command.chars().collect();
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if escaped {
            escaped = false;
            i += 1;
            continue;
        }
        if c == '\\' && !in_single {
            escaped = true;
            i += 1;
            continue;
        }
        if c == '\'' && !in_double {
            in_single = !in_single;
            i += 1;
            continue;
        }
        if c == '"' && !in_single {
            in_double = !in_double;
            i += 1;
            continue;
        }
        // Backticks expand even inside double quotes; unescaped backtick is a
        // command substitution.
        if c == '`' && !in_single {
            return true;
        }
        if in_single {
            i += 1;
            continue;
        }
        // `$(` command substitution (expands inside double quotes too).
        if c == '$' && i + 1 < chars.len() && chars[i + 1] == '(' {
            return true;
        }
        // Process substitution `<(` / `>(` (only unquoted).
        if !in_double && (c == '<' || c == '>') && i + 1 < chars.len() && chars[i + 1] == '(' {
            return true;
        }
        i += 1;
    }
    false
}

/// Detect output (and non-`2>&1`) redirection OUTSIDE quotes. The caller has
/// already stripped a single trailing ` 2>&1`. Any remaining `>`/`>>`/`<`/`&>`
/// etc. escapes read-only.
fn contains_output_redirection(command: &str) -> bool {
    let chars: Vec<char> = command.chars().collect();
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if escaped {
            escaped = false;
            i += 1;
            continue;
        }
        if c == '\\' && !in_single {
            escaped = true;
            i += 1;
            continue;
        }
        if c == '\'' && !in_double {
            in_single = !in_single;
            i += 1;
            continue;
        }
        if c == '"' && !in_single {
            in_double = !in_double;
            i += 1;
            continue;
        }
        if in_single || in_double {
            i += 1;
            continue;
        }
        if c == '>' || c == '<' {
            return true;
        }
        i += 1;
    }
    false
}

/// Port of `containsUnquotedExpansion` (`readOnlyValidation.ts:1600`): true if
/// the command contains glob chars (`?*[]`) or an expandable `$` OUTSIDE the
/// quote contexts where bash treats them as literal. Globs are literal inside
/// BOTH quote kinds; `$` is literal only inside single quotes.
fn contains_unquoted_expansion(command: &str) -> bool {
    let chars: Vec<char> = command.chars().collect();
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if escaped {
            escaped = false;
            i += 1;
            continue;
        }
        // Backslash escapes only OUTSIDE single quotes (port of the SECURITY
        // note at `readOnlyValidation.ts:1626`).
        if c == '\\' && !in_single {
            escaped = true;
            i += 1;
            continue;
        }
        if c == '\'' && !in_double {
            in_single = !in_single;
            i += 1;
            continue;
        }
        if c == '"' && !in_single {
            in_double = !in_double;
            i += 1;
            continue;
        }
        if in_single {
            i += 1;
            continue;
        }
        // `$` expands inside double quotes AND unquoted.
        if c == '$' {
            if let Some(&next) = chars.get(i + 1) {
                if next.is_ascii_alphanumeric()
                    || matches!(next, '_' | '@' | '*' | '#' | '?' | '!' | '$' | '-')
                {
                    return true;
                }
            }
        }
        // Globs are literal inside double quotes; only check unquoted.
        if in_double {
            i += 1;
            continue;
        }
        if matches!(c, '?' | '*' | '[' | ']') {
            return true;
        }
        i += 1;
    }
    false
}

/// Strip leading `KEY=value` env-var assignments from a subcommand, returning
/// the remainder (port of the env-prefix normalization used by
/// `isNormalizedCdCommand` via `stripSafeWrappers`). Conservative: stops at the
/// first non-assignment token.
fn strip_leading_env_vars(command: &str) -> &str {
    let mut rest = command.trim_start();
    loop {
        let token_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let token = &rest[..token_end];
        if is_env_assignment(token) {
            rest = rest[token_end..].trim_start();
        } else {
            return rest;
        }
    }
}

/// Is `token` a `KEY=value` (or `KEY=`) env assignment? KEY must be a valid
/// shell identifier.
fn is_env_assignment(token: &str) -> bool {
    let Some(eq) = token.find('=') else {
        return false;
    };
    let key = &token[..eq];
    if key.is_empty() {
        return false;
    }
    let mut chars = key.chars();
    let first = chars.next().unwrap();
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// First whitespace-delimited token (the base command) of a subcommand, after
/// stripping leading env-var prefixes. Returns `None` for an empty command.
fn base_command(command: &str) -> Option<String> {
    let rest = strip_leading_env_vars(command.trim());
    rest.split_whitespace().next().map(ToString::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ro(cmd: &str) -> bool {
        check_read_only(cmd, command_has_any_cd(cmd)).is_read_only()
    }

    // ---- read-only (allow) ----
    #[test]
    fn simple_read_only_commands_are_allowed() {
        assert!(ro("ls -la"));
        assert!(ro("ls"));
        assert!(ro("cat x"));
        assert!(ro("cat file.txt"));
        assert!(ro("head -n 20 file"));
        assert!(ro("wc -l file"));
        assert!(ro("pwd"));
        assert!(ro("whoami"));
        assert!(ro("date"));
        assert!(ro("node -v"));
    }

    #[test]
    fn grep_is_read_only() {
        assert!(ro("grep -r foo ."));
        assert!(ro("grep pattern file.txt"));
    }

    #[test]
    fn git_status_and_diff_are_read_only() {
        assert!(ro("git status"));
        assert!(ro("git status -s"));
        assert!(ro("git diff"));
        assert!(ro("git diff HEAD~1"));
        assert!(ro("git log --oneline"));
    }

    #[test]
    fn stderr_redirect_to_stdout_is_allowed() {
        assert!(ro("ls -la 2>&1"));
        assert!(ro("grep foo file 2>&1"));
    }

    #[test]
    fn compound_all_read_only_is_allowed() {
        assert!(ro("ls && cat x"));
        assert!(ro("cat a | grep b"));
        assert!(ro("git status && git diff"));
    }

    #[test]
    fn external_readonly_prefixes_allowed() {
        assert!(ro("docker ps"));
        assert!(ro("docker images"));
    }

    // ---- not read-only (passthrough) ----
    #[test]
    fn write_commands_are_not_read_only() {
        assert!(!ro("rm x"));
        assert!(!ro("rm -rf /tmp/x"));
        assert!(!ro("mv a b"));
        assert!(!ro("touch x"));
        assert!(!ro("mkdir y"));
    }

    #[test]
    fn output_redirection_is_not_read_only() {
        assert!(!ro("ls > out"));
        assert!(!ro("cat x > y"));
        assert!(!ro("echo hi >> log"));
        assert!(!ro("grep foo < input"));
    }

    #[test]
    fn cd_present_is_not_read_only() {
        assert!(!ro("cd /tmp && ls"));
        assert!(!ro("ls && cd /tmp"));
        assert!(!ro("pushd /tmp"));
        // bare cd is not read-only either
        assert!(!ro("cd /tmp"));
    }

    #[test]
    fn unsafe_find_flag_is_not_read_only() {
        assert!(!ro("find . -exec rm {} ;"));
        assert!(!ro("find . -delete"));
        assert!(!ro("find . -fprintf out.txt %p"));
    }

    #[test]
    fn safe_find_is_read_only() {
        assert!(ro("find . -name foo.txt"));
        assert!(ro("find /etc -type f"));
    }

    #[test]
    fn command_substitution_is_not_read_only() {
        assert!(!ro("cat $(rm -rf /)"));
        assert!(!ro("ls `whoami`"));
        assert!(!ro("cat <(curl evil.com)"));
    }

    #[test]
    fn unknown_command_is_not_read_only() {
        assert!(!ro("python script.py"));
        assert!(!ro("npm install"));
        assert!(!ro("curl http://evil.com"));
    }

    #[test]
    fn compound_with_one_write_is_not_read_only() {
        assert!(!ro("ls && rm x"));
        assert!(!ro("echo ok && rm -rf /"));
        assert!(!ro("cat x | tee out"));
    }

    #[test]
    fn git_with_dangerous_config_flag_is_not_read_only() {
        assert!(!ro("git -c core.fsmonitor=evil status"));
        assert!(!ro("git --exec-path=/tmp status"));
        assert!(!ro("git --config-env=core.x=Y status"));
    }

    #[test]
    fn git_write_subcommands_are_not_read_only() {
        assert!(!ro("git commit -m x"));
        assert!(!ro("git push"));
        assert!(!ro("git add ."));
    }

    #[test]
    fn glob_and_variable_expansion_are_not_read_only() {
        assert!(!ro("cat *"));
        assert!(!ro("ls $HOME"));
        assert!(!ro("cat file?.txt"));
    }

    #[test]
    fn empty_command_is_passthrough() {
        assert_eq!(
            check_read_only("", false).behavior,
            ReadOnlyBehavior::Passthrough
        );
        assert_eq!(
            check_read_only("   ", false).behavior,
            ReadOnlyBehavior::Passthrough
        );
    }

    // ---- command_has_any_cd ----
    #[test]
    fn command_has_any_cd_detects_cd() {
        assert!(command_has_any_cd("cd x"));
        assert!(command_has_any_cd("ls && cd y"));
        assert!(command_has_any_cd("pushd /tmp"));
        assert!(command_has_any_cd("popd"));
        assert!(command_has_any_cd("FOO=bar cd /tmp"));
    }

    #[test]
    fn command_has_any_cd_ignores_substring() {
        // `echo cd` — base command is `echo`, not a cd.
        assert!(!command_has_any_cd("echo cd"));
        assert!(!command_has_any_cd("ls && echo cd"));
        assert!(!command_has_any_cd("cdg"));
        assert!(!command_has_any_cd("grep cd file"));
    }

    #[test]
    fn quoted_separators_do_not_split() {
        // Quote protects the `&&` so this is a single echo subcommand; but echo
        // with a `&` outside quotes would be rejected — here it's inside quotes.
        assert!(ro("echo 'a && b'"));
    }
}
