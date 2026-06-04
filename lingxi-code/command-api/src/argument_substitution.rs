//! Argument substitution for markdown-defined slash commands.
//!
//! Faithful port of `claude-code/src/utils/argumentSubstitution.ts`. Supports:
//! - `$ARGUMENTS` — replaced with the full (raw) arguments string.
//! - `$ARGUMENTS[0]`, `$ARGUMENTS[1]`, … — individual indexed arguments.
//! - `$0`, `$1`, … — shorthand for `$ARGUMENTS[N]`.
//! - Named arguments (`$foo`, `$bar`) when argument names are declared in
//!   frontmatter.
//!
//! ## Divergence from TS (documented intentionally)
//!
//! * **No `regex` crate.** The crate may not add new dependencies, so the four
//!   TS regexes (`\$<name>(?![\[\w])`, `\$ARGUMENTS\[(\d+)\]`, `\$(\d+)(?!\w)`,
//!   and `replaceAll("$ARGUMENTS", …)`) are reproduced with hand-written
//!   scanners. Each scanner is byte-faithful to the regex it mirrors, including
//!   `\d+`'s greedy-with-backtracking interaction with the `(?!\w)` lookahead
//!   (so `$12` consumes two digits → index 12, while `$12a` matches nothing).
//! * **`parse_arguments` uses the quote-aware tokenizer** from [`crate::parser`]
//!   instead of the `shell-quote` library. This is the "close" substitute the
//!   spec calls for: it strips matched single/double quotes and splits on
//!   whitespace, but (unlike `shell-quote`) does not treat shell operators
//!   (`|`, `;`, `&`) as separate non-string tokens to be filtered out — they
//!   remain part of an adjacent token. `$KEY` variable syntax is preserved
//!   literally (the tokenizer performs no expansion), matching the TS
//!   `tryParseShellCommand(args, key => "$" + key)` behaviour.

use crate::parser::{tokenize_args, ParsedSlashCommand};

/// Frontmatter `arguments` field: either a space-separated string or an array
/// of names. Mirrors the TS `string | string[] | undefined` union.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrontmatterArgs {
    /// Space-separated names in a single string (e.g. `"foo bar baz"`).
    Str(String),
    /// Pre-split list of names (e.g. `["foo", "bar", "baz"]`).
    List(Vec<String>),
}

/// Parse an arguments string into individual arguments.
///
/// Faithful to TS `parseArguments`: empty/whitespace-only input yields an empty
/// vector; otherwise the quote-aware tokenizer splits the string (see the
/// module-level divergence note on `shell-quote`).
#[must_use]
pub fn parse_arguments(args: &str) -> Vec<String> {
    if args.trim().is_empty() {
        return Vec::new();
    }
    tokenize_args(args)
}

/// Parse argument names from the frontmatter `arguments` field.
///
/// Faithful to TS `parseArgumentNames`: filters out empty/whitespace-only names
/// and numeric-only names (which would collide with the `$0`/`$1` shorthand).
#[must_use]
pub fn parse_argument_names(argument_names: Option<&FrontmatterArgs>) -> Vec<String> {
    fn is_valid_name(name: &str) -> bool {
        let trimmed = name.trim();
        !trimmed.is_empty() && !is_all_ascii_digits(name)
    }

    match argument_names {
        None => Vec::new(),
        Some(FrontmatterArgs::List(list)) => list
            .iter()
            .filter(|n| is_valid_name(n))
            .cloned()
            .collect(),
        Some(FrontmatterArgs::Str(s)) => s
            .split_whitespace()
            .filter(|n| is_valid_name(n))
            .map(str::to_string)
            .collect(),
    }
}

/// `true` when `s` is non-empty and every byte is an ASCII digit — the Rust
/// equivalent of the TS `/^\d+$/` test used for the numeric-only filter and the
/// `$ARGUMENTS[N]` / `$N` index extraction.
fn is_all_ascii_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// `true` for `[A-Za-z0-9_]` — the chars matched by the regex `\w` token.
fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Faithful port of TS `substituteArguments`.
///
/// `args == None` (TS `undefined`/`null`) returns `content` unchanged. An empty
/// string is a valid value that replaces placeholders with empty text. Steps run
/// in the exact TS order:
/// 1. named args via `\$<name>(?![\[\w])`,
/// 2. `\$ARGUMENTS\[(\d+)\]`,
/// 3. `\$(\d+)(?!\w)` shorthand,
/// 4. `replaceAll("$ARGUMENTS", args)`,
/// 5. append `\n\nARGUMENTS: {args}` iff nothing changed, `append_if_no_placeholder`,
///    and `args` is non-empty.
#[must_use]
pub fn substitute_arguments_faithful(
    content: &str,
    args: Option<&str>,
    append_if_no_placeholder: bool,
    argument_names: &[String],
) -> String {
    let Some(args) = args else {
        return content.to_string();
    };

    let parsed_args = parse_arguments(args);
    let original_content = content;
    let mut content = content.to_string();

    // (1) Named arguments: $name not followed by `[` or a word char.
    for (i, name) in argument_names.iter().enumerate() {
        if name.is_empty() {
            continue;
        }
        let replacement = parsed_args.get(i).map_or("", String::as_str);
        content = replace_named_arg(&content, name, replacement);
    }

    // (2) Indexed: $ARGUMENTS[<digits>].
    content = replace_arguments_indexed(&content, &parsed_args);

    // (3) Shorthand: $<digits> not followed by a word char (greedy digits).
    content = replace_shorthand_indexed(&content, &parsed_args);

    // (4) Full arguments string.
    content = content.replace("$ARGUMENTS", args);

    // (5) Tail append when nothing was substituted.
    if content == original_content && append_if_no_placeholder && !args.is_empty() {
        content = format!("{content}\n\nARGUMENTS: {args}");
    }

    content
}

/// Replace `$<name>` where `name` is a literal frontmatter argument name and the
/// following char is neither `[` nor a word char. Mirrors the regex
/// `new RegExp("\\$" + name + "(?![\\[\\w])", "g")`.
fn replace_named_arg(content: &str, name: &str, replacement: &str) -> String {
    let bytes = content.as_bytes();
    let name_bytes = name.as_bytes();
    let pat_len = 1 + name_bytes.len(); // '$' + name
    let mut out = String::with_capacity(content.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$'
            && i + pat_len <= bytes.len()
            && &bytes[i + 1..i + pat_len] == name_bytes
        {
            // Negative lookahead `(?![\[\w])` on the char *after* the name.
            let next = bytes.get(i + pat_len).copied();
            let blocked = matches!(next, Some(b'[')) || matches!(next, Some(c) if is_word_byte(c));
            if !blocked {
                out.push_str(replacement);
                i += pat_len;
                continue;
            }
        }
        // SAFETY: `i` always sits on a char boundary — we only ever advance past
        // whole matched ASCII spans or copy one UTF-8 char at a time below.
        let ch_len = utf8_char_len(bytes[i]);
        out.push_str(&content[i..i + ch_len]);
        i += ch_len;
    }
    out
}

/// Replace `$ARGUMENTS[<digits>]` (regex `\$ARGUMENTS\[(\d+)\]`). Out-of-range
/// indices become the empty string (TS `parsedArgs[index] ?? ''`).
fn replace_arguments_indexed(content: &str, parsed_args: &[String]) -> String {
    const PREFIX: &[u8] = b"$ARGUMENTS[";
    let bytes = content.as_bytes();
    let mut out = String::with_capacity(content.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$'
            && i + PREFIX.len() <= bytes.len()
            && &bytes[i..i + PREFIX.len()] == PREFIX
        {
            // Consume one-or-more digits, then require a closing ']'.
            let digits_start = i + PREFIX.len();
            let mut j = digits_start;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > digits_start && j < bytes.len() && bytes[j] == b']' {
                let index: usize = content[digits_start..j].parse().unwrap_or(usize::MAX);
                let replacement = parsed_args.get(index).map_or("", String::as_str);
                out.push_str(replacement);
                i = j + 1; // skip past ']'
                continue;
            }
        }
        let ch_len = utf8_char_len(bytes[i]);
        out.push_str(&content[i..i + ch_len]);
        i += ch_len;
    }
    out
}

/// Replace `$<digits>` shorthand (regex `\$(\d+)(?!\w)`). `\d+` is greedy; the
/// negative lookahead `(?!\w)` then rejects a trailing word char, with the same
/// backtracking the regex engine performs. Concretely:
/// * `$12` → digits `12`, next char is non-word → index 12.
/// * `$12a` → greedy `12` then `a` (word) fails; backtrack to `1`, next is `2`
///   (word) fails; no shorter match → no substitution.
fn replace_shorthand_indexed(content: &str, parsed_args: &[String]) -> String {
    let bytes = content.as_bytes();
    let mut out = String::with_capacity(content.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' {
            // Greedily consume all following digits.
            let digits_start = i + 1;
            let mut end = digits_start;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
            if end > digits_start {
                // Backtrack `end` down to the longest prefix whose following
                // char is not a word char (mirrors regex `(?!\w)` backtracking).
                let mut k = end;
                let mut matched = None;
                while k > digits_start {
                    let next = bytes.get(k).copied();
                    let next_is_word = matches!(next, Some(c) if is_word_byte(c));
                    if !next_is_word {
                        matched = Some(k);
                        break;
                    }
                    k -= 1;
                }
                if let Some(match_end) = matched {
                    let index: usize =
                        content[digits_start..match_end].parse().unwrap_or(usize::MAX);
                    let replacement = parsed_args.get(index).map_or("", String::as_str);
                    out.push_str(replacement);
                    i = match_end;
                    continue;
                }
                // No valid (?!\w) position: emit '$' and rescan from the digits.
                out.push('$');
                i = digits_start;
                continue;
            }
        }
        let ch_len = utf8_char_len(bytes[i]);
        out.push_str(&content[i..i + ch_len]);
        i += ch_len;
    }
    out
}

/// UTF-8 length (in bytes) of the char starting with leading byte `b`.
fn utf8_char_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b >> 5 == 0b110 {
        2
    } else if b >> 4 == 0b1110 {
        3
    } else {
        4
    }
}

/// Backwards-compatible 2-arg shim retained so downstream crates (e.g.
/// `command-core`) keep compiling. Delegates to [`substitute_arguments_faithful`]
/// with `append_if_no_placeholder = true` and no named arguments, passing the
/// raw argument string. The pre-faithful naive behaviour is intentionally
/// dropped in favour of TS parity.
#[must_use]
pub fn substitute_arguments(template: &str, args: &ParsedSlashCommand) -> String {
    substitute_arguments_faithful(template, Some(&args.raw_args), true, &[])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    // ----- parse_arguments -----

    #[test]
    fn parse_arguments_basic_and_quoted() {
        assert_eq!(parse_arguments("foo bar baz"), vec!["foo", "bar", "baz"]);
        assert_eq!(
            parse_arguments("foo \"hello world\" baz"),
            vec!["foo", "hello world", "baz"]
        );
        assert_eq!(
            parse_arguments("foo 'hello world' baz"),
            vec!["foo", "hello world", "baz"]
        );
    }

    #[test]
    fn parse_arguments_empty_and_whitespace() {
        assert!(parse_arguments("").is_empty());
        assert!(parse_arguments("   ").is_empty());
    }

    #[test]
    fn parse_arguments_preserves_dollar_key() {
        // No variable expansion: $KEY stays literal.
        assert_eq!(parse_arguments("$HOME path"), vec!["$HOME", "path"]);
    }

    // ----- parse_argument_names -----

    #[test]
    fn parse_argument_names_string_and_list() {
        assert_eq!(
            parse_argument_names(Some(&FrontmatterArgs::Str("foo bar baz".into()))),
            vec!["foo", "bar", "baz"]
        );
        assert_eq!(
            parse_argument_names(Some(&FrontmatterArgs::List(names(&["foo", "bar"])))),
            vec!["foo", "bar"]
        );
        assert!(parse_argument_names(None).is_empty());
    }

    #[test]
    fn parse_argument_names_filters_empty_and_numeric() {
        assert_eq!(
            parse_argument_names(Some(&FrontmatterArgs::Str("foo  123 bar 7".into()))),
            vec!["foo", "bar"]
        );
        assert_eq!(
            parse_argument_names(Some(&FrontmatterArgs::List(names(&["", "  ", "42", "ok"])))),
            vec!["ok"]
        );
    }

    // ----- indexed / shorthand substitution -----

    #[test]
    fn arguments_indexed_and_shorthand() {
        // $ARGUMENTS[0] and $1 select the same token classes.
        assert_eq!(
            substitute_arguments_faithful("[$ARGUMENTS[0]]", Some("a b c"), true, &[]),
            "[a]"
        );
        assert_eq!(
            substitute_arguments_faithful("[$1]", Some("a b c"), true, &[]),
            "[b]"
        );
        // Out of range -> empty.
        assert_eq!(
            substitute_arguments_faithful("[$ARGUMENTS[9]]", Some("a b"), true, &[]),
            "[]"
        );
    }

    #[test]
    fn shorthand_two_digit_consumes_both_digits() {
        // 13 args so index 12 exists; $12 must mean index 12, not $1 + "2".
        let args = "a0 a1 a2 a3 a4 a5 a6 a7 a8 a9 a10 a11 a12";
        assert_eq!(
            substitute_arguments_faithful("[$12]", Some(args), true, &[]),
            "[a12]"
        );
        // $1 alone still works and is not greedily merged with a following space.
        assert_eq!(
            substitute_arguments_faithful("[$1]", Some(args), true, &[]),
            "[a1]"
        );
    }

    #[test]
    fn shorthand_followed_by_word_char_does_not_match() {
        // `$12a`: greedy `12` rejected by `a`; backtrack to `1` rejected by `2`.
        // No (?!\w) position -> no substitution at all.
        assert_eq!(
            substitute_arguments_faithful("x$12a", Some("a b c d"), false, &[]),
            "x$12a"
        );
        // `$1a` likewise leaves the text untouched.
        assert_eq!(
            substitute_arguments_faithful("$1a", Some("a b"), false, &[]),
            "$1a"
        );
    }

    #[test]
    fn full_arguments_replacement_uses_raw_string() {
        assert_eq!(
            substitute_arguments_faithful("all=$ARGUMENTS", Some("a b c"), true, &[]),
            "all=a b c"
        );
        // $ARGUMENTS[0] is consumed before bare $ARGUMENTS, so the bracket form wins.
        assert_eq!(
            substitute_arguments_faithful("$ARGUMENTS[0]-$ARGUMENTS", Some("a b"), true, &[]),
            "a-a b"
        );
    }

    // ----- named args & (?!\w) boundary -----

    #[test]
    fn named_args_boundary_negative_lookahead() {
        let argnames = names(&["foo"]);
        // $foo replaced; $foobar (word char after) NOT replaced; $foo[0] (next
        // char `[`) NOT replaced — the `(?![\[\w])` lookahead rejects both.
        assert_eq!(
            substitute_arguments_faithful("$foo $foobar $foo[0]", Some("X"), true, &argnames),
            "X $foobar $foo[0]"
        );
    }

    #[test]
    fn named_args_map_by_position() {
        let argnames = names(&["first", "second"]);
        assert_eq!(
            substitute_arguments_faithful(
                "$first then $second",
                Some("alpha beta"),
                true,
                &argnames
            ),
            "alpha then beta"
        );
        // Missing positional -> empty.
        assert_eq!(
            substitute_arguments_faithful("$second", Some("alpha"), false, &argnames),
            ""
        );
    }

    // ----- appendIfNoPlaceholder -----

    #[test]
    fn append_if_no_placeholder_on_appends_when_no_change() {
        assert_eq!(
            substitute_arguments_faithful("no placeholders here", Some("a b"), true, &[]),
            "no placeholders here\n\nARGUMENTS: a b"
        );
    }

    #[test]
    fn append_if_no_placeholder_off_does_not_append() {
        assert_eq!(
            substitute_arguments_faithful("no placeholders here", Some("a b"), false, &[]),
            "no placeholders here"
        );
    }

    #[test]
    fn append_if_no_placeholder_empty_vs_missing_args() {
        // Empty args: no append even though append flag on (TS `&& args` guard).
        assert_eq!(
            substitute_arguments_faithful("plain", Some(""), true, &[]),
            "plain"
        );
        // Missing args (None): content returned unchanged.
        assert_eq!(substitute_arguments_faithful("plain", None, true, &[]), "plain");
        // A placeholder present -> content changes -> no append even with text.
        // $1 is index 1 -> the SECOND token ("b"), matching $ARGUMENTS[1].
        assert_eq!(
            substitute_arguments_faithful("got $1", Some("a b"), true, &[]),
            "got b"
        );
    }

    #[test]
    fn session_id_placeholder_left_untouched() {
        // ${CLAUDE_SESSION_ID} is not an arg placeholder; substitution ignores it.
        // $1 -> index 1 -> "b".
        let out = substitute_arguments_faithful("id=${CLAUDE_SESSION_ID} $1", Some("a b"), true, &[]);
        assert_eq!(out, "id=${CLAUDE_SESSION_ID} b");
    }

    // ----- shim -----

    #[test]
    fn shim_substitutes_positional_and_arguments() {
        let p = crate::parser::parse_slash_command("/foo a b c").unwrap();
        // $1 -> index 1 -> "b" (faithful), $ARGUMENTS -> raw "a b c".
        assert_eq!(
            substitute_arguments("first=$1 all=$ARGUMENTS", &p),
            "first=b all=a b c"
        );
    }
}
