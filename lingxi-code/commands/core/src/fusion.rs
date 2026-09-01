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
    /// Panel preset. Defaults to quality when omitted.
    pub preset: FusionPreset,
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
    let mut prompt_parts = Vec::new();
    let tokens = &args.positional_args;
    let mut i = 0;
    while i < tokens.len() {
        let tok = tokens[i].as_str();
        match tok {
            "--quality" => {
                set_preset(&mut preset, FusionPreset::Quality)?;
                i += 1;
            }
            "--fast" => {
                set_preset(&mut preset, FusionPreset::Fast)?;
                i += 1;
            }
            "--same-provider" => {
                set_cross(&mut cross_provider, false)?;
                i += 1;
            }
            "--cross-provider" => {
                set_cross(&mut cross_provider, true)?;
                i += 1;
            }
            "--partial-ok" => {
                set_once(&mut partial_ok, true, "--partial-ok/--no-partial")?;
                i += 1;
            }
            "--no-partial" => {
                set_once(&mut partial_ok, false, "--partial-ok/--no-partial")?;
                i += 1;
            }
            flag if flag.starts_with("--models=") => {
                set_once(
                    &mut models,
                    parse_models(&flag["--models=".len()..])?,
                    "--models",
                )?;
                i += 1;
            }
            "--models" => {
                i += 1;
                let value = tokens
                    .get(i)
                    .ok_or_else(|| "--models requires a value".to_string())?;
                set_once(&mut models, parse_models(value)?, "--models")?;
                i += 1;
            }
            flag if flag.starts_with("--dimensions=") => {
                set_once(
                    &mut dimensions,
                    parse_dimensions(&flag["--dimensions=".len()..])?,
                    "--dimensions",
                )?;
                i += 1;
            }
            "--dimensions" => {
                i += 1;
                let value = tokens
                    .get(i)
                    .ok_or_else(|| "--dimensions requires a value".to_string())?;
                set_once(&mut dimensions, parse_dimensions(value)?, "--dimensions")?;
                i += 1;
            }
            flag if flag.starts_with("--max-panel=") => {
                set_once(
                    &mut max_panel,
                    parse_max_panel(&flag["--max-panel=".len()..])?,
                    "--max-panel",
                )?;
                i += 1;
            }
            "--max-panel" => {
                i += 1;
                let value = tokens
                    .get(i)
                    .ok_or_else(|| "--max-panel requires a value".to_string())?;
                set_once(&mut max_panel, parse_max_panel(value)?, "--max-panel")?;
                i += 1;
            }
            flag if flag.starts_with("--") => {
                return Err(format!("unknown flag `{flag}`\n{FUSION_SLASH_USAGE}"));
            }
            _ => {
                prompt_parts.extend(tokens[i..].iter().cloned());
                break;
            }
        }
    }
    let prompt = prompt_parts.join(" ").trim().to_string();
    if prompt.is_empty() {
        return Err(FUSION_SLASH_USAGE.to_string());
    }
    Ok(FusionSlashArgs {
        preset: preset.unwrap_or(FusionPreset::Quality),
        cross_provider,
        models,
        dimensions,
        partial_ok,
        max_panel,
        prompt,
    })
}

/// Build a [`FusionRequest`] from parsed slash args and parent session identity.
#[must_use]
pub fn fusion_request_from_slash(
    parsed: FusionSlashArgs,
    parent_profile: String,
    parent_model: String,
    conversation_id: String,
    slash_cross_provider_default: bool,
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
        preset: parsed.preset,
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

fn parse_models(raw: &str) -> Result<Vec<FusionModelRef>, String> {
    let mut out = Vec::new();
    for item in parse_csv(raw, "--models")? {
        let (profile, model) = match item.split_once(':') {
            Some((profile, model)) if !profile.is_empty() && !model.is_empty() => {
                (Some(profile.to_string()), model.to_string())
            }
            Some(_) => return Err(format!("invalid --models entry `{item}`")),
            None => (None, item),
        };
        out.push(FusionModelRef { profile, model });
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
    fn default_is_quality_and_cross_deferred() {
        let args = parse("/fusion review the plan").unwrap();
        assert_eq!(args.preset, FusionPreset::Quality);
        assert_eq!(args.cross_provider, None);
        assert_eq!(args.prompt, "review the plan");
    }

    #[test]
    fn same_provider_and_fast() {
        let args = parse("/fusion --fast --same-provider check locking").unwrap();
        assert_eq!(args.preset, FusionPreset::Fast);
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
            true,
        );
        assert!(req.cross_provider);
        assert_eq!(req.origin, FusionOrigin::Slash);
        assert_eq!(req.conversation_id.as_deref(), Some("conv"));
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
    fn request_uses_partial_default_when_flag_omitted() {
        let parsed = parse("/fusion do the thing").unwrap();
        let req = fusion_request_from_slash(
            parsed,
            "anthropic".into(),
            "claude-sonnet-5".into(),
            "conv".into(),
            true,
            false,
        );
        assert!(!req.partial_ok);
    }
}
