//! Bash read-only command inference — a faithful, conservative port of the CORE
//! of claude-code `src/tools/BashTool/readOnlyValidation.ts`
//! (`checkReadOnlyConstraints` / `isCommandReadOnly`) and its base allowlist
//! `READONLY_COMMANDS` + `READONLY_COMMAND_REGEXES`
//! (`utils/shell/readOnlyCommandValidation.ts`).
//!
//! # What this decides
//! `bashToolHasPermission` step 7 auto-ALLOWS a bash command when EVERY
//! subcommand is read-only (`BashTool.isReadOnly` → `checkReadOnlyConstraints`
//! → `splitCommand_DEPRECATED(command).every(isCommandReadOnly)`). This module
//! reproduces that "all subcommands read-only" inference so [`crate::policy`]
//! can wire the read-only allow layer in TS order (after the allow walk + the
//! sed/mode layers, before the generic ask fallthrough).
//!
//! # Faithful scope vs. documented divergence
//! The TS read-only allowlist is enormous (~1500 lines of git/gh/docker/ripgrep
//! per-subcommand flag configs + getopt-aware flag parsing). Porting that whole
//! machine 1:1 is a separate effort; here we port the BASE [`READONLY_COMMANDS`]
//! simple-command set + the simple-command regex SHAPE — TS
//! `makeRegexForSafeCommand` anchors the base word, then allows a trailing run
//! of characters that excludes the shell metacharacters (redirection, command
//! substitution, pipe, brace/paren group, background, list, newline) — plus the
//! hand-written read-only regexes for echo / pwd / whoami / ls / find / cd /
//! grep / rg that the common case needs.
//!
//! The crucial property is the SAFE DIRECTION: this only ever returns `true`
//! for a command whose base word is on the allowlist AND that carries NO shell
//! metacharacter (any of the chars in [`READONLY_METACHARS`]) — exactly the
//! complement of `makeRegexForSafeCommand`'s trailing character class. Anything
//! with redirection / substitution / a non-allowlisted base word returns
//! `false`, so the command falls through to the normal ask path. A read-only
//! FALSE-negative is harmless (one extra prompt); there is no read-only
//! FALSE-positive that would auto-allow a writing command, because the
//! metacharacter guard rejects every redirection/substitution form and the base
//! word must be an explicitly read-only tool.
//!
//! Per-subcommand splitting reuses [`crate::shell_command::split_command`] (the
//! crate's `splitCommand_DEPRECATED` analogue), matching the TS
//! `splitCommand_DEPRECATED(command).every(...)` structure.

/// Shell metacharacters that disqualify a candidate from the simple-command
/// read-only allowlist — the complement of `makeRegexForSafeCommand`'s trailing
/// character class (which excludes exactly these). A subcommand carrying ANY of
/// these (a redirection, a pipe, a brace/paren group, a background, a list, or a
/// newline) is NOT auto-allowed as read-only.
///
/// These are QUOTE-NAIVE — TS rejects them regardless of quote context too (the
/// `makeRegexForSafeCommand` trailing class excludes them outright). The two
/// expansion metacharacters `$` and backtick are NOT in this set; they are
/// QUOTE-AWARE (literal inside single quotes), handled by
/// [`contains_unquoted_expansion`].
const READONLY_METACHARS: &[char] = &[
    '<', '>', '(', ')', '|', '{', '}', '&', ';', '\n', '\r',
];

/// Base (simple) commands that are read-only — 1:1 with the TS `READONLY_COMMANDS`
/// array (`readOnlyValidation.ts:1432-1499`, the `makeRegexForSafeCommand` set)
/// plus the `EXTERNAL_READONLY_COMMANDS` cross-platform pair folded in as their
/// own base words (`docker` is intentionally NOT a bare base word — only the
/// `docker ps`/`docker images` two-word forms are read-only, handled by
/// [`is_read_only_subcommand`]). Every entry here permits ONLY flag/path
/// arguments free of shell metacharacters.
const READONLY_BASE_COMMANDS: &[&str] = &[
    // Time and date
    "cal", "uptime",
    // File content viewing
    "cat", "head", "tail", "wc", "stat", "strings", "hexdump", "od", "nl",
    // System info
    "id", "uname", "free", "df", "du", "locale", "groups", "nproc",
    // Path information
    "basename", "dirname", "realpath", "readlink",
    // Text processing
    "cut", "paste", "tr", "column", "tac", "rev", "fold", "expand", "unexpand",
    "fmt", "comm", "cmp", "numfmt",
    // File comparison
    "diff",
    // true / false
    "true", "false",
    // Misc. safe commands. (Binary `vho` = `…,"expr","seq","tsort","pr"` — NO
    // `test`/`getconf`; the port previously over-allowed those two read-only.)
    "sleep", "which", "type", "expr", "seq", "tsort", "pr",
    // Hand-written read-only regex commands whose simple forms reduce to a
    // base-word + metachar-free-args shape (`pwd`, `whoami`, `ls`, `find`,
    // `cd`, `arch`, `alias`). `echo`/`grep`/`rg`/`jq`/`uniq`/`history` are
    // handled with their TS-specific guards in `is_read_only_subcommand`.
    "pwd", "whoami", "ls", "find", "cd", "arch", "alias",
];

/// `find` primary actions that WRITE / execute / side-effect — binary `vDp`
/// reject set. A `find` invocation carrying any of these is NOT read-only; most
/// notably `-delete` has no shell metacharacter to trip the generic guard, so
/// without this it would be wrongly auto-allowed.
const FIND_DANGEROUS_ACTIONS: &[&str] = &[
    "-delete",
    "-exec",
    "-execdir",
    "-ok",
    "-okdir",
    "-fprint",
    "-fprint0",
    "-fls",
    "-fprintf",
    "-files0-from",
];

/// Quote-aware scan for an ACTIVE `$` expansion or backtick command
/// substitution, mirroring TS `containsUnquotedExpansion`
/// (`readOnlyValidation.ts:1600`) for `$`, and the backtick arm of
/// `validateDangerousPatterns` (`bashSecurity.ts:853`,
/// `hasUnescapedChar(withDoubleQuotes, backtick)`) for backtick.
///
/// Per bash semantics, `$` and backtick are LITERAL only inside SINGLE quotes;
/// they are ACTIVE (an expansion / command substitution → NOT read-only) when
/// UNQUOTED or inside DOUBLE quotes. The other shell metacharacters stay
/// quote-naive in [`READONLY_METACHARS`]; only these two are quote-aware.
///
/// Walks the (sub)command char by char tracking single/double-quote state,
/// matching the TS tracker exactly:
/// - A backslash escapes the next char ONLY outside single quotes (inside
///   `'...'`, `\` is literal — bash does not escape there).
/// - A single quote toggles single-quote state only when not inside a double
///   quote; a double quote toggles double-quote state only when not inside a
///   single quote.
/// - Inside single quotes everything is literal → skipped.
/// - An unescaped `$` followed by a variable/special-parameter char
///   `[A-Za-z_@*#?!$0-9-]` (TS `containsUnquotedExpansion:1651`) flags an
///   expansion. (`${`/`$(` are caught by the brace/paren metachars instead.)
/// - An unescaped backtick (anywhere not single-quoted) flags command
///   substitution.
///
/// Returns `true` when such an active expansion/substitution is present → the
/// subcommand is NOT read-only.
fn contains_unquoted_expansion(command: &str) -> bool {
    let chars: Vec<char> = command.chars().collect();
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut escaped = false;

    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];

        // Skip the char following an escape (escape only set outside single
        // quotes — see the `\\` arm below).
        if escaped {
            escaped = false;
            i += 1;
            continue;
        }

        // Backslash escapes the next char ONLY outside single quotes (TS
        // `containsUnquotedExpansion:1626`). Inside `'...'`, `\` is literal.
        if c == '\\' && !in_single_quote {
            escaped = true;
            i += 1;
            continue;
        }

        // Quote-state toggles (TS lines 1632-1640).
        if c == '\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
            i += 1;
            continue;
        }
        if c == '"' && !in_single_quote {
            in_double_quote = !in_double_quote;
            i += 1;
            continue;
        }

        // Inside single quotes everything is literal → skip (TS lines 1643-1645).
        if in_single_quote {
            i += 1;
            continue;
        }

        // Active `$` expansion: `$` followed by a variable/special-parameter
        // char. Expands inside double quotes AND unquoted (TS lines 1649-1654).
        if c == '$' {
            if let Some(&next) = chars.get(i + 1) {
                if next.is_ascii_alphanumeric()
                    || matches!(next, '_' | '@' | '*' | '#' | '?' | '!' | '$' | '-')
                {
                    return true;
                }
            }
        }

        // Active backtick command substitution: any unescaped backtick that is
        // not single-quoted (TS `bashSecurity.ts:853`, checked against
        // `withDoubleQuotes` which excludes only single-quoted content → a
        // double-quoted backtick still substitutes).
        if c == '`' {
            return true;
        }

        i += 1;
    }

    false
}

/// Is `command` read-only? — the public entry, mirroring TS
/// `BashTool.isReadOnly` → `checkReadOnlyConstraints` →
/// `splitCommand_DEPRECATED(command).every(isCommandReadOnly)`: the WHOLE
/// command is read-only iff EVERY subcommand is. An empty command is NOT
/// read-only (nothing to allow).
///
/// This is the conservative inference [`crate::policy`] consults for the
/// read-only auto-allow layer (TS `bashToolHasPermission` step 7). It never
/// returns `true` for a command bearing a redirection / substitution / a
/// non-allowlisted base word (see the module docs' safe-direction note).
#[must_use]
pub fn command_is_read_only(command: &str) -> bool {
    let subs = crate::shell_command::split_command(command);
    if subs.is_empty() {
        return false;
    }
    subs.iter().all(|sub| is_read_only_subcommand(sub))
}

/// Is a SINGLE subcommand read-only? — the per-subcommand arm of TS
/// `isCommandReadOnly` (`readOnlyValidation.ts:1678`), in its conservative core.
fn is_read_only_subcommand(sub: &str) -> bool {
    // TS strips a trailing ` 2>&1` before matching (`isCommandReadOnly:1683`).
    let mut test = sub.trim();
    if let Some(stripped) = test.strip_suffix("2>&1") {
        test = stripped.trim_end();
    }
    if test.is_empty() {
        return false;
    }
    // Any quote-naive shell metacharacter (redirection / pipe / group / list /
    // newline) disqualifies — the `makeRegexForSafeCommand` trailing class. TS
    // rejects these regardless of quote context.
    if test.chars().any(|c| READONLY_METACHARS.contains(&c)) {
        return false;
    }
    // Quote-aware `$`/backtick guard: an ACTIVE expansion or command
    // substitution disqualifies (TS `containsUnquotedExpansion` for `$`; the
    // backtick arm of `validateDangerousPatterns`). Both are literal — and so
    // remain read-only — inside SINGLE quotes; active when unquoted or inside
    // double quotes. (A `$`-bearing token can expand to an arbitrary flag at
    // runtime, so TS refuses it too.)
    if contains_unquoted_expansion(test) {
        return false;
    }
    let mut words = test.split_whitespace();
    let Some(base) = words.next() else {
        return false;
    };
    // `docker ps` / `docker images` — the only read-only `docker` forms
    // (`EXTERNAL_READONLY_COMMANDS`). A bare `docker` or any other subcommand is
    // NOT read-only.
    if base == "docker" {
        return matches!(words.next(), Some("ps" | "images"));
    }
    // `echo` with metacharacter-free args is read-only (the TS echo regex; the
    // metachar guard above already rejected the dangerous `` ` ``/`$`/`<>` forms
    // the TS regex excludes).
    if base == "echo" {
        return true;
    }
    // `grep` / `rg` (ripgrep) reading files: flags + paths only, no writes.
    // (ripgrep has no write flag; `grep` likewise only reads. The metachar
    // guard rejects the `>`/`|` forms.)
    if matches!(base, "grep" | "egrep" | "fgrep" | "rg") {
        return true;
    }
    // `history` — bare or with a numeric argument only (TS
    // `/^history(?:\s+\d+)?\s*$/`).
    if base == "history" {
        return match words.next() {
            None => true,
            Some(arg) => words.next().is_none() && arg.bytes().all(|b| b.is_ascii_digit()),
        };
    }
    // `find` is read-only ONLY when it carries no destructive/side-effecting
    // action (binary `vDp`). The generic metachar guard catches `-exec … {} \;`
    // forms (via `{`/`}`/`;`), but `-delete` and bare `-fls`/`-fprint*` slip
    // through metachar-free, so reject them explicitly.
    if base == "find" {
        return !words.any(|w| FIND_DANGEROUS_ACTIONS.contains(&w));
    }
    READONLY_BASE_COMMANDS.contains(&base)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Binary `vho` read-only base set excludes `test`/`getconf`; the port
    // previously over-allowed them.
    #[test]
    fn test_and_getconf_are_not_read_only() {
        assert!(!command_is_read_only("test -f foo"));
        assert!(!command_is_read_only("getconf PAGE_SIZE"));
    }

    // `find` is read-only only without a destructive/side-effecting action
    // (binary `vDp`). `-delete` carries no shell metachar, so it would slip past
    // the generic guard without the explicit reject.
    #[test]
    fn find_read_only_only_without_dangerous_actions() {
        assert!(command_is_read_only("find . -name foo.rs"));
        assert!(command_is_read_only("find src -type f"));
        assert!(!command_is_read_only("find . -delete"));
        assert!(!command_is_read_only("find . -name x -delete"));
        assert!(!command_is_read_only("find . -fls out.txt"));
        assert!(!command_is_read_only("find . -fprint out.txt"));
        assert!(!command_is_read_only("find . -files0-from list"));
    }

    #[test]
    fn core_read_commands_are_read_only() {
        assert!(command_is_read_only("cat foo.txt"));
        assert!(command_is_read_only("ls -la"));
        assert!(command_is_read_only("grep pattern file.rs"));
        assert!(command_is_read_only("pwd"));
        assert!(command_is_read_only("whoami"));
        assert!(command_is_read_only("head -n 5 a.log"));
        assert!(command_is_read_only("wc -l src/main.rs"));
        assert!(command_is_read_only("rg needle"));
        assert!(command_is_read_only("echo hello world"));
        assert!(command_is_read_only("find . -name '*.rs'"));
    }

    #[test]
    fn compound_all_read_only_is_read_only() {
        assert!(command_is_read_only("cat a && ls -l"));
        assert!(command_is_read_only("pwd; whoami"));
        // a pipe makes the WHOLE command carry a `|` metachar in each split's
        // boundary? No — split removes the `|`, but `cat x | grep y` splits into
        // `cat x` and `grep y`, both read-only.
        assert!(command_is_read_only("cat x | grep y"));
    }

    #[test]
    fn writing_or_unknown_commands_are_not_read_only() {
        assert!(!command_is_read_only("rm -rf /tmp/x"));
        assert!(!command_is_read_only("curl https://x"));
        assert!(!command_is_read_only("npm install"));
        assert!(!command_is_read_only("mkdir foo"));
        // A read command compounded with a writer is NOT read-only overall.
        assert!(!command_is_read_only("cat a && rm b"));
    }

    #[test]
    fn redirection_and_substitution_are_not_read_only() {
        // A redirect target makes the subcommand carry `>` → not read-only.
        assert!(!command_is_read_only("cat a > out.txt"));
        assert!(!command_is_read_only("echo hi > /etc/x"));
        // Command substitution / process substitution.
        assert!(!command_is_read_only("cat $(echo a)"));
        assert!(!command_is_read_only("echo `whoami`"));
        // `$VAR` expansion can smuggle a flag → refused.
        assert!(!command_is_read_only("cat $FILE"));
    }

    #[test]
    fn trailing_stderr_dup_is_tolerated() {
        // `cat x 2>&1` reduces to `cat x` (the `2>&1` is stripped first) → still
        // read-only. (But a non-trailing `2>&1` keeps a `>` metachar → not.)
        assert!(command_is_read_only("cat x 2>&1"));
    }

    #[test]
    fn docker_only_ps_and_images() {
        assert!(command_is_read_only("docker ps"));
        assert!(command_is_read_only("docker images"));
        assert!(!command_is_read_only("docker run x"));
        assert!(!command_is_read_only("docker"));
    }

    #[test]
    fn history_numeric_only() {
        assert!(command_is_read_only("history"));
        assert!(command_is_read_only("history 20"));
        assert!(!command_is_read_only("history -c"));
    }

    #[test]
    fn empty_command_is_not_read_only() {
        assert!(!command_is_read_only(""));
        assert!(!command_is_read_only("   "));
    }
}

#[cfg(test)]
mod test_quote_aware_expansion {
    //! Parity for the quote-aware `$`/backtick handling — mirroring TS
    //! `containsUnquotedExpansion` (`readOnlyValidation.ts:1600`) and the
    //! backtick arm of `validateDangerousPatterns`
    //! (`bashSecurity.ts:853`). Single-quoted `$`/backtick are LITERAL (still
    //! read-only); double-quoted or unquoted are ACTIVE (NOT read-only).
    use super::*;

    #[test]
    fn single_quoted_dollar_is_literal_and_read_only() {
        // Single-quoted `$VAR` is literal in bash → read-only (TS allows; the
        // former quote-naive `$` membership test wrongly denied this).
        assert!(command_is_read_only("echo '$VAR'"));
        assert!(command_is_read_only("grep '$x' file"));
    }

    #[test]
    fn double_quoted_dollar_expands_not_read_only() {
        // Double-quoted `$VAR` expands at runtime → NOT read-only.
        assert!(!command_is_read_only("echo \"$VAR\""));
    }

    #[test]
    fn unquoted_dollar_expands_not_read_only() {
        // Unquoted `$VAR` expands → NOT read-only.
        assert!(!command_is_read_only("echo $VAR"));
        // `$VAR` expansion can smuggle a flag → refused.
        assert!(!command_is_read_only("cat $FILE"));
    }

    #[test]
    fn single_quoted_backtick_is_literal_and_read_only() {
        // Single-quoted backtick is literal in bash → read-only.
        assert!(command_is_read_only("echo '`id`'"));
    }

    #[test]
    fn double_quoted_backtick_substitutes_not_read_only() {
        // Double-quoted backtick is command substitution → NOT read-only
        // (TS checks backtick against `withDoubleQuotes`, which excludes only
        // single-quoted content).
        assert!(!command_is_read_only("echo \"`id`\""));
        // Unquoted backtick likewise.
        assert!(!command_is_read_only("echo `whoami`"));
    }

    #[test]
    fn compound_quoted_read_then_writer_is_not_read_only() {
        // The `cat '$a'` subcommand is read-only (single-quoted `$` literal),
        // but the `rm x` subcommand fails the read-only base set → the whole
        // compound is NOT read-only.
        assert!(!command_is_read_only("cat '$a'; rm x"));
    }

    #[test]
    fn arithmetic_expansion_still_rejected_via_paren_metachars() {
        // `$((1+1))` is no longer rejected by the `$` guard (the char after
        // `$` is `(`, not a name char), but the `(`/`)` quote-naive metachars
        // still disqualify it → NOT read-only, matching TS (which catches the
        // parens via COMMAND_SUBSTITUTION_PATTERNS).
        assert!(!command_is_read_only("echo $((1+1))"));
    }
}
