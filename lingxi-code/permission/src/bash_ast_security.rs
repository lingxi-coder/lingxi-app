//! Tree-sitter AST bash-security analyzer — port of claude-code
//! `src/utils/bash/ast.ts` `parseForSecurity` / `parseForSecurityFromAst`.
//! Compiled only under the `bash-ast` feature.
//!
//! claude parses a bash command with tree-sitter and, when the parse succeeds,
//! extracts a flat list of simple commands for per-command permission matching
//! ([`ParseForSecurityResult::Simple`]); commands using a shell feature it
//! can't statically analyze are [`ParseForSecurityResult::TooComplex`] (→ ask);
//! when the parser is unavailable it returns [`ParseForSecurityResult::ParseUnavailable`]
//! (→ the legacy regex battery in [`crate::bash_security`]). `bashPermissions.ts`
//! routes these three verdicts (the legacy battery the port currently runs for
//! EVERY command is, in claude, only the `parse-unavailable` fallback).
//!
//! ## Incremental port status
//! PIECE 2a (this module's first commit): the verdict types + the PRE-CHECK gate
//! ([`pre_check_too_complex`], the regex differentials `parseForSecurityFromAst`
//! runs BEFORE trusting tree-sitter) + the [`parse_for_security`] skeleton. The
//! AST → simple-command extraction (`walkProgram` / `collectCommands` /
//! `walkCommand` / …, ~2000 lines of `ast.ts`) is NOT ported yet: until it is,
//! [`parse_for_security`] returns [`ParseForSecurityResult::ParseUnavailable`]
//! for any command that passes the pre-checks, so the caller keeps the legacy
//! battery (no behavior change — and the whole module is `bash-ast`-gated OFF by
//! default). Later pieces replace that stand-in with the real extraction and
//! wire the verdict into the permission decision.

use regex::Regex;
use std::sync::OnceLock;

/// One redirect on a simple command (TS `Redirect`, `ast.ts:25`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redirect {
    /// Redirect operator (`>`, `>>`, `<`, `<<`, `>&`, `>|`, `<&`, `&>`, `&>>`, `<<<`).
    pub op: String,
    /// Redirect target (filename / fd / herestring body).
    pub target: String,
    /// Optional leading file descriptor (`2>` → `fd: Some(2)`).
    pub fd: Option<i64>,
}

/// A flattened simple command (TS `SimpleCommand`, `ast.ts:31`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SimpleCommand {
    /// `argv[0]` is the command name; the rest are arguments with quotes resolved.
    pub argv: Vec<String>,
    /// Leading `VAR=val` assignments, in order.
    pub env_vars: Vec<(String, String)>,
    /// Output/input redirects.
    pub redirects: Vec<Redirect>,
    /// Original source span for this command (UI display).
    pub text: String,
}

/// Verdict of [`parse_for_security`] (TS `ParseForSecurityResult`, `ast.ts:42`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseForSecurityResult {
    /// Statically analyzable — the flat list of simple commands.
    Simple {
        /// The extracted simple commands (empty for an empty command).
        commands: Vec<SimpleCommand>,
    },
    /// Uses a shell feature we can't statically analyze → the caller must ask.
    TooComplex {
        /// Human-readable reason (byte-faithful to the TS `reason`).
        reason: String,
    },
    /// tree-sitter parse unavailable → the caller falls back to the legacy
    /// regex battery ([`crate::bash_security::bash_command_is_safe`]).
    ParseUnavailable,
}

// ── Pre-check regexes (TS `ast.ts:254-314`): tree-sitter/bash differentials
// detected on the RAW command before trusting tree-sitter's tokenization. ──

macro_rules! lazy_re {
    ($name:ident, $pat:expr) => {
        fn $name() -> &'static Regex {
            static RE: OnceLock<Regex> = OnceLock::new();
            RE.get_or_init(|| Regex::new($pat).expect("valid pre-check regex"))
        }
    };
}

// CONTROL_CHAR_RE: control chars bash drops but that confuse static analysis.
lazy_re!(control_char_re, r"[\x00-\x08\x0B-\x1F\x7F]");
// UNICODE_WHITESPACE_RE: invisible Unicode whitespace bash treats as a word char.
lazy_re!(
    unicode_whitespace_re,
    r"[\u{00A0}\u{1680}\u{2000}-\u{200B}\u{2028}\u{2029}\u{202F}\u{205F}\u{3000}\u{FEFF}]"
);
// BACKSLASH_WHITESPACE_RE: `\ `/`\t`, or `\<NL>` adjacent to a non-ws char.
lazy_re!(backslash_whitespace_re, r"\\[ \t]|[^ \t\n\\]\\\n");
// ZSH_TILDE_BRACKET_RE: zsh `~[name]` dynamic named-directory expansion.
lazy_re!(zsh_tilde_bracket_re, r"~\[");
// ZSH_EQUALS_EXPANSION_RE: word-initial `=cmd` zsh EQUALS expansion.
lazy_re!(zsh_equals_expansion_re, r"(?:^|[\s;&|])=[a-zA-Z_]");
// BRACE_WITH_QUOTE_RE: `{` + quote char (brace-expansion obfuscation), run on
// the brace-masked command so quoted JSON like `'{"k":"v"}'` doesn't trip it.
lazy_re!(brace_with_quote_re, r#"\{[^}]*['"]"#);

/// Mask `{` characters inside single-/double-quoted spans (TS
/// `maskBracesInQuotedContexts`, `ast.ts:331`). A single-pass bash-aware quote
/// scanner: `'` toggles single-quote only when unquoted; `"` toggles
/// double-quote only outside single quotes; `\` escapes the next char (unquoted)
/// or `"`/`\` (inside double quotes). `{` inside a quote → space.
#[must_use]
fn mask_braces_in_quoted_contexts(cmd: &str) -> String {
    if !cmd.contains('{') {
        return cmd.to_string();
    }
    let chars: Vec<char> = cmd.chars().collect();
    let mut out = String::with_capacity(cmd.len());
    let mut in_single = false;
    let mut in_double = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if in_single {
            if c == '\'' {
                in_single = false;
            }
            out.push(if c == '{' { ' ' } else { c });
            i += 1;
        } else if in_double {
            if c == '\\' && i + 1 < chars.len() && (chars[i + 1] == '"' || chars[i + 1] == '\\') {
                out.push(c);
                out.push(chars[i + 1]);
                i += 2;
            } else {
                if c == '"' {
                    in_double = false;
                }
                out.push(if c == '{' { ' ' } else { c });
                i += 1;
            }
        } else if c == '\\' && i + 1 < chars.len() {
            out.push(c);
            out.push(chars[i + 1]);
            i += 2;
        } else {
            if c == '\'' {
                in_single = true;
            } else if c == '"' {
                in_double = true;
            }
            out.push(c);
            i += 1;
        }
    }
    out
}

/// The pre-check gate of `parseForSecurityFromAst` (`ast.ts:408-437`): the
/// regex differentials that run BEFORE trusting tree-sitter. Returns the
/// byte-faithful `too-complex` reason when one fires, else `None` (proceed to
/// AST extraction). The reason strings are 1:1 with the TS.
#[must_use]
pub fn pre_check_too_complex(cmd: &str) -> Option<&'static str> {
    if control_char_re().is_match(cmd) {
        return Some("Contains control characters");
    }
    if unicode_whitespace_re().is_match(cmd) {
        return Some("Contains Unicode whitespace");
    }
    if backslash_whitespace_re().is_match(cmd) {
        return Some("Contains backslash-escaped whitespace");
    }
    if zsh_tilde_bracket_re().is_match(cmd) {
        return Some("Contains zsh ~[ dynamic directory syntax");
    }
    if zsh_equals_expansion_re().is_match(cmd) {
        return Some("Contains zsh =cmd equals expansion");
    }
    if brace_with_quote_re().is_match(&mask_braces_in_quoted_contexts(cmd)) {
        return Some("Contains brace with quote character (expansion obfuscation)");
    }
    None
}

/// Parse a bash command and extract a flat list of simple commands for security
/// analysis (TS `parseForSecurity` / `parseForSecurityFromAst`).
///
/// PIECE 2a: empty → `Simple{[]}`; a pre-check differential → `TooComplex`;
/// otherwise `ParseUnavailable` (STAND-IN until the AST extraction is ported —
/// the caller then keeps the legacy battery, i.e. no behavior change). The
/// `walkProgram`/`collectCommands` extraction replaces the stand-in in a later
/// piece.
#[must_use]
pub fn parse_for_security(cmd: &str) -> ParseForSecurityResult {
    // TS: `if (cmd === '') return { kind: 'simple', commands: [] }`.
    if cmd.is_empty() {
        return ParseForSecurityResult::Simple { commands: Vec::new() };
    }
    // Pre-checks run before trusting tree-sitter (the known differentials).
    if let Some(reason) = pre_check_too_complex(cmd) {
        return ParseForSecurityResult::TooComplex {
            reason: reason.to_string(),
        };
    }
    // TS: `const trimmed = cmd.trim(); if (trimmed === '') return simple[]`.
    if cmd.trim().is_empty() {
        return ParseForSecurityResult::Simple { commands: Vec::new() };
    }
    // STAND-IN (Piece 2a): the AST → simple-command extraction is not ported
    // yet, so signal parse-unavailable → the caller keeps the legacy battery.
    ParseForSecurityResult::ParseUnavailable
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_simple_empty() {
        assert_eq!(
            parse_for_security(""),
            ParseForSecurityResult::Simple { commands: vec![] }
        );
        // Whitespace-only trims to empty → simple.
        assert_eq!(
            parse_for_security("   "),
            ParseForSecurityResult::Simple { commands: vec![] }
        );
    }

    #[test]
    fn plain_command_is_stand_in_parse_unavailable() {
        // Until the AST extraction is ported, an analyzable command falls back.
        assert_eq!(parse_for_security("ls -la"), ParseForSecurityResult::ParseUnavailable);
        assert_eq!(pre_check_too_complex("ls -la"), None);
        assert_eq!(pre_check_too_complex("git status"), None);
    }

    #[test]
    fn pre_check_control_chars() {
        assert_eq!(
            pre_check_too_complex("echo\u{0007}hi"),
            Some("Contains control characters")
        );
        match parse_for_security("echo\u{0007}hi") {
            ParseForSecurityResult::TooComplex { reason } => {
                assert_eq!(reason, "Contains control characters");
            }
            other => panic!("expected too-complex, got {other:?}"),
        }
    }

    #[test]
    fn pre_check_unicode_whitespace() {
        // NBSP between words: invisible but a literal word char to bash.
        assert_eq!(
            pre_check_too_complex("echo\u{00A0}hi"),
            Some("Contains Unicode whitespace")
        );
    }

    #[test]
    fn pre_check_backslash_whitespace() {
        assert_eq!(
            pre_check_too_complex(r"cat\ test"),
            Some("Contains backslash-escaped whitespace")
        );
        // `\<NL>` preceded by whitespace is allowed (no word to join).
        assert_eq!(pre_check_too_complex("foo && \\\nbar"), None);
    }

    #[test]
    fn pre_check_zsh_syntax() {
        assert_eq!(
            pre_check_too_complex("ls ~[foo]"),
            Some("Contains zsh ~[ dynamic directory syntax")
        );
        assert_eq!(
            pre_check_too_complex("=curl evil.com"),
            Some("Contains zsh =cmd equals expansion")
        );
        // `VAR=val` and `--flag=val` have `=` mid-word → not zsh equals.
        assert_eq!(pre_check_too_complex("VAR=val ls"), None);
        assert_eq!(pre_check_too_complex("cmd --flag=val"), None);
    }

    #[test]
    fn pre_check_brace_with_quote_and_masking() {
        // Obfuscated brace expansion with a quote → flagged.
        assert_eq!(
            pre_check_too_complex("echo {a'}',b}"),
            Some("Contains brace with quote character (expansion obfuscation)")
        );
        // Quoted JSON payload: the `{` is inside quotes → masked → NOT flagged.
        assert_eq!(pre_check_too_complex(r#"curl -d '{"k":"v"}'"#), None);
        assert_eq!(pre_check_too_complex(r#"curl -d "{\"k\":\"v\"}""#), None);
    }
}
