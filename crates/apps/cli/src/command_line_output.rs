use crate::argv::Argv;
use std::ffi::OsString;

/// First missing required argument's bare name from a clap
/// `MissingRequiredArgument` error, with clap's `<…>`/`[…]`/`...` usage
/// decoration stripped — so callers can render commander's
/// `error: missing required argument '<name>'` (claude-code parity). clap lists
/// the missing args in declaration order; commander reports only the first.
pub(super) fn first_missing_required_arg(e: &clap::Error) -> Option<String> {
    use clap::error::{ContextKind, ContextValue};
    let raw = match e.get(ContextKind::InvalidArg)? {
        ContextValue::Strings(v) => v.first()?.clone(),
        ContextValue::String(s) => s.clone(),
        _ => return None,
    };
    let cleaned = raw
        .trim()
        .trim_end_matches("...")
        .trim_matches(|c| c == '<' || c == '>' || c == '[' || c == ']')
        .to_string();
    (!cleaned.is_empty()).then_some(cleaned)
}

/// One value out of a clap error context slot (the first, if it is a list).
pub(super) fn ctx_string(e: &clap::Error, kind: clap::error::ContextKind) -> Option<String> {
    match e.get(kind)? {
        clap::error::ContextValue::String(s) => Some(s.clone()),
        clap::error::ContextValue::Strings(v) => v.first().cloned(),
        _ => None,
    }
}

/// Damerau-Levenshtein edit distance, faithful to commander's `editDistance`
/// (suggestSimilar.js): includes the transposition rule AND the early-out
/// `|len(a)-len(b)| > maxDistance ⇒ max(len)` so clap's different default
/// metric can't pick a different "Did you mean" candidate than the oracle.
pub(super) fn edit_distance(a: &str, b: &str, max_distance: usize) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (la, lb) = (a.len(), b.len());
    if la.abs_diff(lb) > max_distance {
        return la.max(lb);
    }
    let mut d = vec![vec![0usize; lb + 1]; la + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for j in 0..=lb {
        d[0][j] = j;
    }
    for i in 1..=la {
        for j in 1..=lb {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut m = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                m = m.min(d[i - 2][j - 2] + 1);
            }
            d[i][j] = m;
        }
    }
    d[la][lb]
}

/// commander's `suggestSimilar`: among `candidates`, keep those with similarity
/// `(maxLen-dist)/maxLen > 0.4` at the minimum edit distance (≤ 3), sorted; emit
/// `(Did you mean X?)` for one or `(Did you mean one of A, B?)` for several.
/// `None` when nothing is close enough (commander then prints no suggestion).
pub(super) fn suggest_similar(word: &str, candidates: &[String]) -> Option<String> {
    const MAX_DISTANCE: usize = 3;
    const MIN_SIMILARITY: f64 = 0.4;
    let mut seen = std::collections::HashSet::new();
    let mut best: Vec<String> = Vec::new();
    let mut best_distance = MAX_DISTANCE;
    for cand in candidates {
        if !seen.insert(cand.as_str()) || cand.chars().count() <= 1 {
            continue;
        }
        let distance = edit_distance(word, cand, MAX_DISTANCE);
        let length = word.chars().count().max(cand.chars().count());
        if length == 0 {
            continue;
        }
        let similarity = (length - distance) as f64 / length as f64;
        if similarity > MIN_SIMILARITY {
            if distance < best_distance {
                best_distance = distance;
                best = vec![cand.clone()];
            } else if distance == best_distance {
                best.push(cand.clone());
            }
        }
    }
    best.sort();
    match best.len() {
        0 => None,
        1 => Some(format!("(Did you mean {}?)", best[0])),
        _ => Some(format!("(Did you mean one of {}?)", best.join(", "))),
    }
}

/// Subcommand names valid at the point where an invalid subcommand was typed —
/// the candidate set for [`suggest_similar`]. Walks the clap command tree along
/// the subcommand tokens in `args` up to the bad token, then lists that node's
/// subcommands (matching commander's candidate set, which includes `help`).
pub(super) fn invalid_subcommand_candidates(args: &[OsString], bad: &str) -> Vec<String> {
    use clap::CommandFactory;
    let mut cmd = Argv::command();
    for tok in args.iter().skip(1) {
        let t = tok.to_string_lossy();
        if t == bad {
            break;
        }
        if let Some(sub) = cmd.find_subcommand(t.as_ref()) {
            cmd = sub.clone();
        }
    }
    cmd.get_subcommands()
        .map(|s| s.get_name().to_string())
        .collect()
}

/// Reformat the clap argv errors that claude-code (commander) renders
/// differently, to commander's exact single-/two-line form (stderr, exit 1).
/// Returns `None` for kinds we leave to clap's own rendering (the remaining
/// clap-vs-commander help-block layout difference).
pub(super) fn commander_error(e: &clap::Error, args: &[OsString]) -> Option<String> {
    use clap::error::{ContextKind, ErrorKind};
    match e.kind() {
        // `error: missing required argument '<name>'` (FIRST missing positional).
        ErrorKind::MissingRequiredArgument => {
            first_missing_required_arg(e).map(|n| format!("error: missing required argument '{n}'"))
        }
        // `error: unknown option '--flag'`. clap also raises `UnknownArgument`
        // for EXCESS POSITIONALS, but commander silently ignores those — so only
        // reformat when the offending token is a flag (`-`-prefixed); a bare
        // positional falls through to clap (excess-positional parity is separate).
        ErrorKind::UnknownArgument => {
            let arg = ctx_string(e, ContextKind::InvalidArg)?;
            arg.starts_with('-')
                .then(|| format!("error: unknown option '{arg}'"))
        }
        // `error: unknown command '<cmd>'` + optional `(Did you mean <x>?)`. The
        // suggestion is computed with commander's own algorithm/candidate set
        // (NOT clap's, which picks different candidates — e.g. `ad`⇒`add-json`
        // vs commander's `add`).
        ErrorKind::InvalidSubcommand => {
            let cmd = ctx_string(e, ContextKind::InvalidSubcommand)?;
            let mut msg = format!("error: unknown command '{cmd}'");
            let candidates = invalid_subcommand_candidates(args, &cmd);
            if let Some(s) = suggest_similar(&cmd, &candidates) {
                msg.push('\n');
                msg.push_str(&s);
            }
            Some(msg)
        }
        // `error: option '<flag> <placeholder>' argument '<value>' is invalid.
        // Allowed choices are <choices>.` — clap's choices (`ValidValue`) are
        // already in declared order, matching commander. clap renders an
        // optional-value placeholder as `[<x>]`; commander uses `[x]`, so strip
        // the inner angle brackets.
        ErrorKind::InvalidValue => {
            let flag = ctx_string(e, ContextKind::InvalidArg)?
                .replace("[<", "[")
                .replace(">]", "]");
            let value = ctx_string(e, ContextKind::InvalidValue)?;
            let choices = match e.get(ContextKind::ValidValue)? {
                clap::error::ContextValue::Strings(v) => v.clone(),
                clap::error::ContextValue::String(s) => vec![s.clone()],
                _ => return None,
            };
            Some(format!(
                "error: option '{flag}' argument '{value}' is invalid. Allowed choices are {}.",
                choices.join(", ")
            ))
        }
        _ => None,
    }
}

/// Normalize clap's visible-alias layout to commander's option-heading layout.
///
/// clap renders a visible alias as a detached `[aliases: ...]` paragraph while
/// commander renders both spellings in the option heading.  The aliases below
/// are part of Claude Code's public root-help contract, so keep their accepted
/// parser spellings *and* present them in the same place in `--help` output.
/// Move the `Usage:` block ahead of the description, as commander renders it.
///
/// The usage block is the `Usage:` line plus any following INDENTED
/// continuation lines (clap wraps long usage strings that way); taking only the
/// first line would strip the tail of a wrapped usage onto the wrong side of
/// the description.
pub(super) fn normalise_preamble(help: &str) -> String {
    let lines: Vec<&str> = help.lines().collect();
    let Some(start) = lines.iter().position(|l| l.starts_with("Usage:")) else {
        return help.to_string();
    };
    if start == 0 {
        return help.to_string();
    }
    let mut end = start + 1;
    while end < lines.len()
        && lines[end].starts_with(char::is_whitespace)
        && !lines[end].trim().is_empty()
    {
        end += 1;
    }
    let usage = &lines[start..end];
    let before = &lines[..start];
    let after = &lines[end..];
    let mut out: Vec<&str> = Vec::with_capacity(lines.len() + 1);
    out.extend_from_slice(usage);
    out.push("");
    out.extend(before.iter().copied());
    out.extend(after.iter().copied());
    let joined = out.join("\n");
    let mut joined = joined;
    while joined.contains("\n\n\n") {
        joined = joined.replace("\n\n\n", "\n\n");
    }
    joined
}

/// Reorder clap's help sections into commander's order.
///
/// clap emits `Commands:` before `Arguments:`/`Options:`; commander emits
/// `Arguments:` -> `Options:` -> `Commands:`. This is a pure text transform on
/// the RENDERED help rather than a `help_template`, because a template must be
/// written per command: `{all-args}` cannot be reordered, and spelling the
/// sections out individually would print a bare `Arguments:` header for the
/// ~40 subcommands that have no positionals.
///
/// Only sections that are actually present move, so a command with no
/// positionals still emits no `Arguments:` header. Anything that is not one of
/// the three known sections keeps its position relative to the preamble, so an
/// unrecognised block cannot be silently dropped.
pub(super) fn reorder_help_sections(help: &str) -> String {
    // A section header is a line at column 0 ending in ':' — clap's own format.
    fn is_header(line: &str) -> bool {
        !line.starts_with(char::is_whitespace)
            && line.ends_with(':')
            && line.len() > 1
            && line.starts_with(|c: char| c.is_ascii_uppercase())
    }

    // Normalise the PREAMBLE first: commander leads with `Usage:` and puts the
    // description after it; clap leads with the description on every
    // subcommand. `help_template` is not inherited by subcommands in clap
    // derive, so doing this on the rendered text covers all ~50 command paths
    // with one mechanism instead of an attribute on every struct.
    let help = &normalise_preamble(help);
    let lines: Vec<&str> = help.lines().collect();
    let first = lines.iter().position(|l| is_header(l));
    let Some(first) = first else {
        return help.to_string();
    };
    // `Usage:` is part of the preamble, not a movable section.
    let mut preamble: Vec<&str> = lines[..first].to_vec();
    let mut sections: Vec<(String, Vec<&str>)> = Vec::new();
    let mut cur: Option<(String, Vec<&str>)> = None;
    for line in &lines[first..] {
        if is_header(line) {
            if let Some(sec) = cur.take() {
                sections.push(sec);
            }
            cur = Some(((*line).to_string(), Vec::new()));
        } else if let Some((_, body)) = cur.as_mut() {
            body.push(line);
        }
    }
    if let Some(sec) = cur.take() {
        sections.push(sec);
    }
    // `Usage:` renders as a header but belongs with the preamble.
    while sections
        .first()
        .is_some_and(|(h, _)| h.starts_with("Usage:"))
    {
        let (h, body) = sections.remove(0);
        preamble.push(Box::leak(h.into_boxed_str()));
        preamble.extend(body);
    }

    let rank = |h: &str| match h {
        _ if h.starts_with("Arguments:") => 0,
        _ if h.starts_with("Options:") => 1,
        _ if h.starts_with("Commands:") => 2,
        _ => 3,
    };
    sections.sort_by_key(|(h, _)| rank(h));

    let mut out = preamble.join("\n");
    for (h, body) in sections {
        // Exactly one blank line before every section header, as commander
        // renders it. Rebuilding the blocks drops whatever spacing they had.
        while out.ends_with('\n') {
            out.pop();
        }
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&h);
        out.push('\n');
        out.push_str(&body.join("\n"));
        out.push('\n');
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

pub(super) fn commander_help(e: &clap::Error) -> String {
    let mut help = e.to_string();
    for (canonical, alias) in [
        ("--allowedTools", "--allowed-tools"),
        ("--disallowedTools", "--disallowed-tools"),
        ("--bg", "--background"),
    ] {
        let heading = format!("{canonical}, {alias}");
        help = help.replacen(canonical, &heading, 1);
        let alias_paragraph = format!(
            "\n          \n          [aliases: {}]",
            alias.trim_start_matches("--")
        );
        help = help.replace(&alias_paragraph, "");
    }
    reorder_help_sections(&help)
}
