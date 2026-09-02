//! `/fusion` argument parser.
//!
//! Not a builtin name — the handler is registered at the desktop composition
//! root so [`command_api::builtin_support::names::BUILTIN_COMMAND_NAMES`] stays
//! locked at 108.

use command_api::parser::ParsedSlashCommand;
use platform_api::{
    normalize_dimensions, FusionModelRef, FusionOrigin, FusionPreset, FusionRequest,
    DEFAULT_FUSION_DIMENSIONS, FUSION_MAX_PANEL, FUSION_MIN_PANEL, FUSION_SCHEMA_VERSION,
};

/// Usage line for empty / invalid invocations.
pub const FUSION_SLASH_USAGE: &str = "Usage: /fusion [--quality|--fast] [--same-provider|--cross-provider] [--models profile:model,...] [--dimensions dim,...] [--partial-ok|--no-partial] [--max-panel N] PROMPT";

/// Parsed `/fusion` flags plus the remaining prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FusionSlashArgs {
    /// Explicit panel preset. `None` = use the runtime Fusion default.
    pub preset: Option<FusionPreset>,
    /// `None` = use `slashCrossProviderDefault`.
    pub cross_provider: Option<bool>,
    /// Explicit panel models.
    pub models: Option<Vec<FusionModelRef>>,
    /// Scoring dimensions.
    pub dimensions: Option<Vec<String>>,
    /// Continue when some panels fail.
    pub partial_ok: Option<bool>,
    /// Panel cap override.
    pub max_panel: Option<u8>,
    /// Non-empty task prompt.
    pub prompt: String,
}

/// Parse `/fusion` tokens. Unknown flags and empty prompts are errors.
///
/// # Errors
///
/// Returns [`FUSION_SLASH_USAGE`] or a more specific mutex / parse error.
pub fn parse_fusion_slash(args: &ParsedSlashCommand) -> Result<FusionSlashArgs, String> {
    let mut preset = None;
    let mut cross_provider = None;
    let mut models = None;
    let mut dimensions = None;
    let mut partial_ok = None;
    let mut max_panel = None;
    let tokens = &args.positional_args;
    let raw = args.raw_args.as_str();
    // `tokens` is shell-quote output: unquoted `*`/`?` globs are dropped,
    // `#` truncates the remainder as a comment, `( ) | & ; < >` are operators.
    // That is fine for recognising leading `--flag` words (always plain
    // ASCII), but the prompt itself must come back from `raw_args` verbatim
    // — so we only use `tokens` to find where the flags end, then recover
    // everything after that point directly from the untokenized string,
    // preserving punctuation and newlines the tokenizer would have mangled.
    let mut cursor = 0usize;
    let mut i = 0;
    while i < tokens.len() {
        let tok = tokens[i].as_str();
        match tok {
            "--quality" => {
                set_preset(&mut preset, FusionPreset::Quality)?;
                cursor = advance_past_token(raw, cursor, tok);
                i += 1;
            }
            "--fast" => {
                set_preset(&mut preset, FusionPreset::Fast)?;
                cursor = advance_past_token(raw, cursor, tok);
                i += 1;
            }
            "--same-provider" => {
                set_cross(&mut cross_provider, false)?;
                cursor = advance_past_token(raw, cursor, tok);
                i += 1;
            }
            "--cross-provider" => {
                set_cross(&mut cross_provider, true)?;
                cursor = advance_past_token(raw, cursor, tok);
                i += 1;
            }
            "--partial-ok" => {
                set_once(&mut partial_ok, true, "--partial-ok/--no-partial")?;
                cursor = advance_past_token(raw, cursor, tok);
                i += 1;
            }
            "--no-partial" => {
                set_once(&mut partial_ok, false, "--partial-ok/--no-partial")?;
                cursor = advance_past_token(raw, cursor, tok);
                i += 1;
            }
            flag if flag.starts_with("--models=") => {
                set_once(
                    &mut models,
                    parse_models(&flag["--models=".len()..])?,
                    "--models",
                )?;
                cursor = advance_past_token(raw, cursor, tok);
                i += 1;
            }
            "--models" => {
                cursor = advance_past_token(raw, cursor, tok);
                i += 1;
                let value = tokens
                    .get(i)
                    .ok_or_else(|| "--models requires a value".to_string())?;
                set_once(&mut models, parse_models(value)?, "--models")?;
                cursor = advance_past_token(raw, cursor, value);
                i += 1;
            }
            flag if flag.starts_with("--dimensions=") => {
                set_once(
                    &mut dimensions,
                    parse_dimensions(&flag["--dimensions=".len()..])?,
                    "--dimensions",
                )?;
                cursor = advance_past_token(raw, cursor, tok);
                i += 1;
            }
            "--dimensions" => {
                cursor = advance_past_token(raw, cursor, tok);
                i += 1;
                let value = tokens
                    .get(i)
                    .ok_or_else(|| "--dimensions requires a value".to_string())?;
                set_once(&mut dimensions, parse_dimensions(value)?, "--dimensions")?;
                cursor = advance_past_token(raw, cursor, value);
                i += 1;
            }
            flag if flag.starts_with("--max-panel=") => {
                set_once(
                    &mut max_panel,
                    parse_max_panel(&flag["--max-panel=".len()..])?,
                    "--max-panel",
                )?;
                cursor = advance_past_token(raw, cursor, tok);
                i += 1;
            }
            "--max-panel" => {
                cursor = advance_past_token(raw, cursor, tok);
                i += 1;
                let value = tokens
                    .get(i)
                    .ok_or_else(|| "--max-panel requires a value".to_string())?;
                set_once(&mut max_panel, parse_max_panel(value)?, "--max-panel")?;
                cursor = advance_past_token(raw, cursor, value);
                i += 1;
            }
            flag if flag.starts_with("--") => {
                return Err(format!("unknown flag `{flag}`\n{FUSION_SLASH_USAGE}"));
            }
            _ => break,
        }
    }
    let prompt = raw[cursor.min(raw.len())..].trim().to_string();
    if prompt.is_empty() {
        return Err(FUSION_SLASH_USAGE.to_string());
    }
    Ok(FusionSlashArgs {
        preset,
        cross_provider,
        models,
        dimensions,
        partial_ok,
        max_panel,
        prompt,
    })
}

/// Advance `cursor` past the next literal occurrence of `token` in `raw`,
/// searching from `cursor` onward. `token` is a value pulled from the
/// shell-quote tokenizer's output, so it is always a substring of the raw
/// text at that position (quotes/backslashes it stripped surround it, but
/// don't split it). If it can't be found (should not happen for well-formed
/// input), fall back to skipping to the next whitespace boundary so parsing
/// still makes forward progress instead of looping.
fn advance_past_token(raw: &str, cursor: usize, token: &str) -> usize {
    let cursor = cursor.min(raw.len());
    let rest = &raw[cursor..];
    if let Some(rel) = rest.find(token) {
        let mut end = cursor + rel + token.len();
        // If the raw text quoted this token (e.g. `--models "a,b"`), the
        // shell-quote tokenizer already stripped the surrounding quotes from
        // `token` before handing it to us, so `token` matches only the
        // INSIDE of the raw quoted span — skip the closing quote too, or it
        // leaks as the first character of the recovered prompt.
        let opened_with_quote = rel > 0
            && matches!(rest.as_bytes().get(rel - 1), Some(b'"' | b'\''));
        if opened_with_quote {
            let opening = rest.as_bytes()[rel - 1];
            if raw.as_bytes().get(end) == Some(&opening) {
                end += 1;
            }
        }
        return end;
    }
    let trimmed_start = rest
        .find(|c: char| !c.is_whitespace())
        .unwrap_or(rest.len());
    let after_start = &rest[trimmed_start..];
    let word_end = after_start
        .find(char::is_whitespace)
        .unwrap_or(after_start.len());
    cursor + trimmed_start + word_end
}

/// Build a [`FusionRequest`] from parsed slash args and parent session identity.
#[must_use]
pub fn fusion_request_from_slash(
    parsed: FusionSlashArgs,
    parent_profile: String,
    parent_model: String,
    conversation_id: String,
    slash_cross_provider_default: bool,
    default_preset: FusionPreset,
    default_partial_ok: bool,
) -> FusionRequest {
    let dimensions = parsed.dimensions.unwrap_or_else(|| {
        DEFAULT_FUSION_DIMENSIONS
            .iter()
            .map(|s| (*s).to_string())
            .collect()
    });
    FusionRequest {
        schema_version: FUSION_SCHEMA_VERSION,
        origin: FusionOrigin::Slash,
        prompt: parsed.prompt,
        preset: parsed.preset.unwrap_or(default_preset),
        models: parsed.models,
        dimensions,
        partial_ok: parsed.partial_ok.unwrap_or(default_partial_ok),
        max_panel: parsed.max_panel,
        cross_provider: parsed
            .cross_provider
            .unwrap_or(slash_cross_provider_default),
        parent_profile,
        parent_model,
        conversation_id: Some(conversation_id),
        workflow_run_id: None,
    }
}

fn set_preset(slot: &mut Option<FusionPreset>, value: FusionPreset) -> Result<(), String> {
    if let Some(existing) = *slot {
        if existing == value {
            let flag = match value {
                FusionPreset::Quality => "--quality",
                FusionPreset::Fast => "--fast",
            };
            return Err(format!("{flag} specified more than once"));
        }
        return Err("--quality and --fast are mutually exclusive".into());
    }
    *slot = Some(value);
    Ok(())
}

fn set_cross(slot: &mut Option<bool>, value: bool) -> Result<(), String> {
    if let Some(existing) = *slot {
        if existing == value {
            let flag = if value {
                "--cross-provider"
            } else {
                "--same-provider"
            };
            return Err(format!("{flag} specified more than once"));
        }
        return Err("--same-provider and --cross-provider are mutually exclusive".into());
    }
    *slot = Some(value);
    Ok(())
}

fn set_once<T>(slot: &mut Option<T>, value: T, flag: &str) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!("{flag} specified more than once"));
    }
    *slot = Some(value);
    Ok(())
}

fn parse_csv(raw: &str, flag: &str) -> Result<Vec<String>, String> {
    let values = raw
        .split(',')
        .map(str::trim)
        .map(str::to_string)
        .collect::<Vec<_>>();
    if values.is_empty() || values.iter().any(String::is_empty) {
        return Err(format!("{flag} must not contain empty entries"));
    }
    Ok(values)
}

/// Splits `raw` on commas, then parses each entry through the single
/// `platform_api::parse_fusion_model_ref` the Agent tool and workflow
/// `fusion()`'s preset parsing also route through, so a malformed entry (e.g.
/// `openai:` — a colon with an empty model) is rejected identically from
/// every entrypoint.
fn parse_models(raw: &str) -> Result<Vec<FusionModelRef>, String> {
    let mut out = Vec::new();
    for item in parse_csv(raw, "--models")? {
        out.push(
            platform_api::parse_fusion_model_ref(&item)
                .map_err(|error| error.to_string())?,
        );
    }
    if out.len() < usize::from(FUSION_MIN_PANEL) {
        return Err("explicit --models must contain at least 2 entries".into());
    }
    Ok(out)
}

fn parse_dimensions(raw: &str) -> Result<Vec<String>, String> {
    normalize_dimensions(parse_csv(raw, "--dimensions")?).map_err(|error| error.to_string())
}

fn parse_max_panel(raw: &str) -> Result<u8, String> {
    let n: u8 = raw
        .parse()
        .map_err(|_| format!("--max-panel `{raw}` is not an integer"))?;
    if !(FUSION_MIN_PANEL..=FUSION_MAX_PANEL).contains(&n) {
        return Err(format!(
            "--max-panel must be {FUSION_MIN_PANEL}..={FUSION_MAX_PANEL}"
        ));
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use command_api::parse_slash_command;

    fn parse(line: &str) -> Result<FusionSlashArgs, String> {
        let parsed = parse_slash_command(line).expect("slash");
        parse_fusion_slash(&parsed)
    }

    #[test]
    fn quoted_flag_value_does_not_leak_its_closing_quote_into_the_prompt() {
        let args = parse(
            "/fusion --models \"openai:gpt-5,anthropic:opus\" review the plan",
        )
        .unwrap();
        assert_eq!(args.prompt, "review the plan");
    }

    #[test]
    fn empty_prompt_is_usage() {
        assert_eq!(parse("/fusion").unwrap_err(), FUSION_SLASH_USAGE);
        assert_eq!(parse("/fusion --quality").unwrap_err(), FUSION_SLASH_USAGE);
    }

    #[test]
    fn quality_and_fast_are_mutex() {
        let err = parse("/fusion --quality --fast review this").unwrap_err();
        assert!(err.contains("mutually exclusive"), "{err}");
    }

    #[test]
    fn same_and_cross_are_mutex() {
        let err = parse("/fusion --same-provider --cross-provider review this").unwrap_err();
        assert!(err.contains("mutually exclusive"), "{err}");
    }

    #[test]
    fn omitted_preset_and_cross_provider_are_deferred_to_runtime_settings() {
        let args = parse("/fusion review the plan").unwrap();
        assert_eq!(args.preset, None);
        assert_eq!(args.cross_provider, None);
        assert_eq!(args.prompt, "review the plan");
    }

    #[test]
    fn same_provider_and_fast() {
        let args = parse("/fusion --fast --same-provider check locking").unwrap();
        assert_eq!(args.preset, Some(FusionPreset::Fast));
        assert_eq!(args.cross_provider, Some(false));
        assert_eq!(args.prompt, "check locking");
    }

    #[test]
    fn models_and_dimensions() {
        let args = parse(
            "/fusion --models anthropic:claude-sonnet-5,openai:gpt-5.6-terra --dimensions coverage,reasoning review",
        )
        .unwrap();
        assert_eq!(args.models.as_ref().map(Vec::len), Some(2));
        assert_eq!(
            args.dimensions.as_deref(),
            Some(["coverage".to_string(), "reasoning".to_string()].as_slice())
        );
    }

    #[test]
    fn unknown_flag_rejected() {
        let err = parse("/fusion --nope review").unwrap_err();
        assert!(err.contains("unknown flag"), "{err}");
    }

    #[test]
    fn request_uses_slash_cross_default() {
        let parsed = parse("/fusion do the thing").unwrap();
        let req = fusion_request_from_slash(
            parsed,
            "anthropic".into(),
            "claude-sonnet-5".into(),
            "conv".into(),
            true,
            FusionPreset::Fast,
            true,
        );
        assert!(req.cross_provider);
        assert_eq!(req.preset, FusionPreset::Fast);
        assert_eq!(req.origin, FusionOrigin::Slash);
        assert_eq!(req.conversation_id.as_deref(), Some("conv"));
    }

    #[test]
    fn a_colon_with_an_empty_model_is_rejected_with_the_same_message_as_the_agent_tool() {
        // `platform_api::parse_fusion_model_ref` is the single implementation
        // both `/fusion` and the Agent tool (`tools/agent/src/agent.rs`'s
        // `parse_fusion_models`) route a `--models`/`models[]` entry through,
        // so an invalid entry rejects identically from either entrypoint.
        let err = parse("/fusion --models openai:,anthropic:opus review").unwrap_err();
        assert!(
            err.contains("invalid fusion request: invalid fusion models entry `openai:`"),
            "{err}"
        );
    }

    #[test]
    fn duplicate_models_flag_is_rejected() {
        let err = parse("/fusion --models a,b --models c,d review").unwrap_err();
        assert!(err.contains("specified more than once"), "{err}");
    }

    #[test]
    fn duplicate_same_provider_flag_is_rejected() {
        let err = parse("/fusion --same-provider --same-provider review").unwrap_err();
        assert!(err.contains("specified more than once"), "{err}");
    }

    #[test]
    fn empty_csv_entries_are_rejected() {
        assert!(parse("/fusion --models a,,b review")
            .unwrap_err()
            .contains("empty entries"));
        assert!(parse("/fusion --dimensions coverage, review")
            .unwrap_err()
            .contains("empty entries"));
        assert!(parse("/fusion --dimensions= review")
            .unwrap_err()
            .contains("empty entries"));
    }

    #[test]
    fn dimensions_are_validated_and_deduplicated_before_spawn() {
        let args = parse("/fusion --dimensions coverage,coverage,reasoning review").unwrap();
        assert_eq!(
            args.dimensions,
            Some(vec!["coverage".to_string(), "reasoning".to_string()])
        );
        assert!(parse("/fusion --dimensions Not-Snake review").is_err());
    }

    #[test]
    fn max_panel_accepts_only_the_locked_range() {
        assert_eq!(
            parse("/fusion --max-panel 2 review").unwrap().max_panel,
            Some(2)
        );
        assert_eq!(
            parse("/fusion --max-panel=8 review").unwrap().max_panel,
            Some(8)
        );
        assert!(parse("/fusion --max-panel 1 review").is_err());
        assert!(parse("/fusion --max-panel 9 review").is_err());
    }

    #[test]
    fn prompt_recovers_verbatim_text_shell_quote_would_mangle() {
        // Bare `*` `?` are dropped as globs, `#` truncates as a comment and
        // `(` `)` `|` etc. are operators in the shell-quote tokenizer that
        // `positional_args` goes through — the prompt must NOT be rebuilt
        // from those tokens, it must come back from `raw_args` verbatim.
        let args = parse("/fusion what does foo(bar) do?").unwrap();
        assert_eq!(args.prompt, "what does foo(bar) do?");

        let args = parse("/fusion review PR #123 for races").unwrap();
        assert_eq!(args.prompt, "review PR #123 for races");

        let args = parse("/fusion find a * b").unwrap();
        assert_eq!(args.prompt, "find a * b");
    }

    #[test]
    fn prompt_preserves_newlines_after_leading_flags() {
        let args = parse("/fusion --quality line one\nline two").unwrap();
        assert_eq!(args.prompt, "line one\nline two");
    }

    #[test]
    fn flags_before_punctuation_heavy_prompt_are_still_consumed() {
        let args =
            parse("/fusion --fast --same-provider --dimensions coverage what about foo(bar)?")
                .unwrap();
        assert_eq!(args.preset, Some(FusionPreset::Fast));
        assert_eq!(args.cross_provider, Some(false));
        assert_eq!(args.prompt, "what about foo(bar)?");
    }

    #[test]
    fn request_uses_partial_default_when_flag_omitted() {
        let parsed = parse("/fusion do the thing").unwrap();
        let req = fusion_request_from_slash(
            parsed,
            "anthropic".into(),
            "claude-sonnet-5".into(),
            "conv".into(),
            true,
            FusionPreset::Quality,
            false,
        );
        assert!(!req.partial_ok);
    }
}
