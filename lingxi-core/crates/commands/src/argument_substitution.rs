//! Argument substitution for markdown-defined slash commands.
//!
//! Supports `$ARGUMENTS` (raw argument string), `$@` (space-joined positional
//! args), and `$1..$N` (individual positional args).

use crate::parser::ParsedSlashCommand;

/// Replace placeholder tokens in `template` with values from `args`.
#[must_use]
pub fn substitute_arguments(template: &str, args: &ParsedSlashCommand) -> String {
    let mut out = template.to_string();
    out = out.replace("$ARGUMENTS", &args.raw_args);
    out = out.replace("$@", &args.positional_args.join(" "));
    for (i, a) in args.positional_args.iter().enumerate() {
        out = out.replace(&format!("${}", i + 1), a);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutes_positional_and_arguments() {
        let p = crate::parser::parse_slash_command("/foo a b c").unwrap();
        assert_eq!(
            substitute_arguments("first=$1 all=$ARGUMENTS", &p),
            "first=a all=a b c"
        );
    }
}
