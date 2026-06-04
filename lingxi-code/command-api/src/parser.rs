//! Slash-command parser: splits raw input like `/memory add foo bar` into
//! a name and tokenized argument list, honouring quoted strings.

/// A parsed slash-command invocation.
#[derive(Debug, Clone)]
pub struct ParsedSlashCommand {
    /// Command name (without the leading `/`).
    pub name: String,
    /// The raw argument string after the command name.
    pub raw_args: String,
    /// Argument tokens after quote-aware splitting.
    pub positional_args: Vec<String>,
}

/// Parse a slash-command line.
///
/// Returns `None` if the input does not begin with `/`.
#[must_use]
pub fn parse_slash_command(input: &str) -> Option<ParsedSlashCommand> {
    let trimmed_input = input.strip_prefix('/')?;
    let (name, args_str) = match trimmed_input.find(char::is_whitespace) {
        Some(i) => (&trimmed_input[..i], trimmed_input[i + 1..].trim_start()),
        None => (trimmed_input, ""),
    };
    let positional = tokenize_args(args_str);
    Some(ParsedSlashCommand {
        name: name.to_string(),
        raw_args: args_str.to_string(),
        positional_args: positional,
    })
}

/// Split a string into tokens, respecting single- and double-quoted runs.
///
/// Shared with [`crate::argument_substitution::parse_arguments`] as the
/// quote-aware substitute for the TS `shell-quote` tokenizer.
#[allow(clippy::match_same_arms)] // arm order is significant — guards must run first.
pub(crate) fn tokenize_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut in_quote: Option<char> = None;
    for c in s.chars() {
        match (in_quote, c) {
            (Some(q), c) if c == q => {
                in_quote = None;
                out.push(std::mem::take(&mut buf));
            }
            (Some(_), c) => buf.push(c),
            (None, '"' | '\'') => {
                in_quote = Some(c);
            }
            (None, c) if c.is_whitespace() => {
                if !buf.is_empty() {
                    out.push(std::mem::take(&mut buf));
                }
            }
            (None, c) => buf.push(c),
        }
    }
    if !buf.is_empty() {
        out.push(buf);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_name_and_args() {
        let p = parse_slash_command("/memory add foo bar").unwrap();
        assert_eq!(p.name, "memory");
        assert_eq!(p.positional_args, vec!["add", "foo", "bar"]);
        assert_eq!(p.raw_args, "add foo bar");
    }

    #[test]
    fn handles_quoted_args() {
        let p = parse_slash_command("/skill run \"git commit\"").unwrap();
        assert_eq!(p.positional_args, vec!["run", "git commit"]);
    }
}
