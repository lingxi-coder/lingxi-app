//! Offline-first Fusion evaluation entrypoint.
//!
//! Default mode is a deterministic dry-run. `--replay <path>` reads a
//! sanitized saved output and computes provenance/format/objective metrics
//! without network access. `--live` is intentionally fail-closed in this
//! foundation until a real host adapter is supplied by a later PR.

#[path = "../evaluation/mod.rs"]
mod evaluation;

use evaluation::harness::{
    dry_run, replay_json, validate_live_options, EvalError, LiveOptions, MAX_LIVE_BUDGET_NANO_USD,
    MAX_LIVE_RUNS, MAX_REPLAY_JSON_BYTES,
};
use std::env;
use std::fs;
use std::io::Read;

fn usage() -> &'static str {
    "fusion_eval [--dry-run] | [--replay PATH] | [--live --paid-opt-in --run-count N --budget-nano-usd N]\n\n\
Default mode enumerates all offline fixture comparisons. Live mode is fail-closed until a real host adapter is wired."
}

fn main() {
    if let Err(error) = run(env::args().skip(1).collect()) {
        eprintln!("fusion_eval error: {error}");
        std::process::exit(2);
    }
}

fn read_replay_file(path: &str) -> Result<String, EvalError> {
    let file = fs::File::open(path).map_err(|error| EvalError::Io(error.to_string()))?;
    let mut bytes = Vec::new();
    file.take((MAX_REPLAY_JSON_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| EvalError::Io(error.to_string()))?;
    if bytes.len() > MAX_REPLAY_JSON_BYTES {
        return Err(EvalError::InvalidSavedRun(format!(
            "replay file exceeds the {MAX_REPLAY_JSON_BYTES}-byte limit"
        )));
    }
    String::from_utf8(bytes)
        .map_err(|_| EvalError::InvalidSavedRun("replay file must be valid UTF-8".into()))
}

fn run(args: Vec<String>) -> Result<(), EvalError> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("{}", usage());
        return Ok(());
    }
    let mut dry = false;
    let mut replay_path: Option<String> = None;
    let mut live = false;
    let mut options = LiveOptions::default();
    let mut index = 0usize;
    while index < args.len() {
        match args[index].as_str() {
            "--dry-run" => dry = true,
            "--live" => live = true,
            "--paid-opt-in" => options.paid_opt_in = true,
            "--replay" => {
                index += 1;
                replay_path = Some(
                    args.get(index)
                        .ok_or_else(|| EvalError::InvalidInput("--replay needs a path".into()))?
                        .clone(),
                );
            }
            "--run-count" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| EvalError::InvalidInput("--run-count needs a value".into()))?;
                options.run_count = Some(value.parse().map_err(|_| {
                    EvalError::InvalidInput("--run-count must be an integer".into())
                })?);
            }
            "--budget-nano-usd" => {
                index += 1;
                let value = args.get(index).ok_or_else(|| {
                    EvalError::InvalidInput("--budget-nano-usd needs a value".into())
                })?;
                options.budget_nano_usd = Some(value.parse().map_err(|_| {
                    EvalError::InvalidInput("--budget-nano-usd must be an integer".into())
                })?);
            }
            unknown => {
                return Err(EvalError::InvalidInput(format!(
                    "unknown argument {unknown}"
                )));
            }
        }
        index += 1;
    }

    let has_live_flags =
        options.paid_opt_in || options.run_count.is_some() || options.budget_nano_usd.is_some();
    if live && (dry || replay_path.is_some()) {
        return Err(EvalError::InvalidInput(
            "--live cannot be combined with --dry-run or --replay".into(),
        ));
    }
    if !live && has_live_flags {
        return Err(EvalError::InvalidInput(
            "--paid-opt-in/--run-count/--budget-nano-usd require --live".into(),
        ));
    }
    if live {
        // No adapter is intentionally available in this foundation. Validate
        // all operator caps first, then fail closed without touching a provider.
        validate_live_options(&options)?;
        return Err(EvalError::LiveAdapterUnavailable);
    }
    if replay_path.is_some() && dry {
        return Err(EvalError::InvalidInput(
            "choose one of --dry-run or --replay".into(),
        ));
    }
    if let Some(path) = replay_path {
        let input = read_replay_file(&path)?;
        let report = replay_json(&input)?;
        println!(
            "{}",
            serde_json::to_string_pretty(&report)
                .map_err(|error| EvalError::InvalidInput(error.to_string()))?
        );
        return Ok(());
    }
    let report = dry_run()?;
    println!(
        "{}",
        serde_json::to_string_pretty(&report)
            .map_err(|error| EvalError::InvalidInput(error.to_string()))?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_run_is_offline_dry_run() {
        run(Vec::new()).expect("default dry-run");
    }

    #[test]
    fn live_requires_explicit_args_and_then_fails_closed_without_adapter() {
        assert!(matches!(
            run(vec!["--live".into()]),
            Err(EvalError::MissingPaidOptIn)
        ));
        assert!(matches!(
            run(vec![
                "--live".into(),
                "--paid-opt-in".into(),
                "--run-count".into(),
                (MAX_LIVE_RUNS + 1).to_string(),
                "--budget-nano-usd".into(),
                MAX_LIVE_BUDGET_NANO_USD.to_string()
            ]),
            Err(EvalError::RunCountExceeded { .. })
        ));
        assert!(matches!(
            run(vec![
                "--live".into(),
                "--paid-opt-in".into(),
                "--run-count".into(),
                "1".into(),
                "--budget-nano-usd".into(),
                "1".into()
            ]),
            Err(EvalError::LiveAdapterUnavailable)
        ));
    }

    #[test]
    fn live_flags_without_live_are_rejected_instead_of_ignored() {
        for flag in [
            vec!["--paid-opt-in".into()],
            vec!["--run-count".into(), "1".into()],
            vec!["--budget-nano-usd".into(), "1".into()],
        ] {
            assert!(matches!(run(flag), Err(EvalError::InvalidInput(_))));
        }
    }

    #[test]
    fn live_does_not_supersede_other_modes() {
        let with_dry_run = vec![
            "--live".into(),
            "--dry-run".into(),
            "--paid-opt-in".into(),
            "--run-count".into(),
            "1".into(),
            "--budget-nano-usd".into(),
            "1".into(),
        ];
        assert!(matches!(run(with_dry_run), Err(EvalError::InvalidInput(_))));

        let with_replay = vec![
            "--live".into(),
            "--replay".into(),
            "saved.json".into(),
            "--paid-opt-in".into(),
            "--run-count".into(),
            "1".into(),
            "--budget-nano-usd".into(),
            "1".into(),
        ];
        assert!(matches!(run(with_replay), Err(EvalError::InvalidInput(_))));

        assert!(matches!(
            run(vec![
                "--dry-run".into(),
                "--replay".into(),
                "saved.json".into()
            ]),
            Err(EvalError::InvalidInput(_))
        ));
    }
}
