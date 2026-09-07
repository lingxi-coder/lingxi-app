//! App-tier evaluation command. Defaults to zero-network dry-run.
use clap::Args;
use engine_desktop::fusion_evaluation::{
    self,
    evaluation::harness::{self, LiveOptions},
    LiveSelection,
};
use std::sync::Arc;

#[derive(Debug, Clone, Args)]
pub struct Cli {
    #[arg(long, conflicts_with_all=["live","runtime_check"])]
    pub dry_run: bool,
    #[arg(long, conflicts_with_all=["live","dry_run"])]
    pub runtime_check: bool,
    #[arg(long)]
    pub live: bool,
    #[arg(long, requires = "live")]
    pub paid_opt_in: bool,
    #[arg(long, requires = "live")]
    pub run_count: Option<usize>,
    #[arg(long, requires = "live")]
    pub budget_nano_usd: Option<u64>,
    #[arg(long, requires = "live")]
    pub max_model_calls: Option<u32>,
    #[arg(long, value_delimiter = ',', requires = "live")]
    pub comparisons: Vec<usize>,
}

pub async fn run(cli: &Cli, argv: Option<&crate::argv::Argv>) -> i32 {
    match execute(cli, argv).await {
        Ok((value, success)) => match serde_json::to_string_pretty(&value) {
            Ok(json) => {
                println!("{json}");
                if success {
                    0
                } else {
                    1
                }
            }
            Err(error) => {
                eprintln!("fusion-eval report error: {error}");
                2
            }
        },
        Err(error) => {
            eprintln!("fusion-eval: {error}");
            2
        }
    }
}

async fn execute(
    cli: &Cli,
    argv: Option<&crate::argv::Argv>,
) -> Result<(serde_json::Value, bool), String> {
    if !cli.live {
        if cli.dry_run && cli.runtime_check {
            return Err("choose one offline mode".into());
        }
        if cli.paid_opt_in
            || cli.run_count.is_some()
            || cli.budget_nano_usd.is_some()
            || cli.max_model_calls.is_some()
            || !cli.comparisons.is_empty()
        {
            return Err("paid opt-in/caps/selections require --live".into());
        }
        let value = if cli.runtime_check {
            serde_json::to_value(
                fusion_evaluation::evaluation::runtime::runtime_report()
                    .await
                    .map_err(|error| error.to_string())?,
            )
        } else {
            serde_json::to_value(harness::dry_run().map_err(|error| error.to_string())?)
        };
        return value
            .map(|value| (value, true))
            .map_err(|error| error.to_string());
    }
    if cli.dry_run || cli.runtime_check {
        return Err("live cannot be combined with offline modes".into());
    }
    let selection = LiveSelection::validate(
        &LiveOptions {
            paid_opt_in: cli.paid_opt_in,
            run_count: cli.run_count,
            budget_nano_usd: cli.budget_nano_usd,
        },
        cli.max_model_calls,
        &cli.comparisons,
    )
    .map_err(|error| error.to_string())?;
    let default_argv;
    let argv = match argv {
        Some(argv) => argv,
        None => {
            default_argv = crate::argv::Argv::from_iter(["lingxi-cli", "--print"])
                .map_err(|error| error.to_string())?;
            &default_argv
        }
    };
    if argv.resume.is_some()
        || argv.continue_session
        || argv.fork_session
        || argv.session_id.is_some()
        || argv.no_session_persistence
        || argv.resume_session_at.is_some()
        || argv.resume_drops_turn.is_some()
    {
        return Err("evaluation forbids resume/fork/session override/ephemeral options".into());
    }
    let mut cfg = crate::init::resolve_desktop_config(argv, permission::PermissionMode::Default);
    cfg.session_persistence = true;
    let output = Arc::new(crate::output_adapter::SinkAdapter::new(Arc::new(EvalSink)));
    let report = fusion_evaluation::run_live(
        selection,
        cfg,
        output,
        Arc::new(crate::init::NoopPermissionRequestSink),
    )
    .await?;
    let success = report.succeeded();
    serde_json::to_value(report)
        .map(|value| (value, success))
        .map_err(|error| error.to_string())
}

struct EvalSink;
#[async_trait::async_trait]
impl crate::output::OutputSink for EvalSink {
    async fn text(&self, _: &str) {}
    async fn turn_start(&self) {}
    async fn turn_end(&self, _: &str, _: f64, _: u64, _: u64) {}
    async fn tool_call(&self, _: &str, _: &serde_json::Value) {}
    async fn tool_result(&self, _: &str, _: &serde_json::Value) {}
    async fn tool_heartbeat(&self, _: &str, _: &str, _: u64) {}
    async fn command_output(&self, _: &str, _: &str) {}
    async fn error(&self, code: &str, message: &str) {
        eprintln!("fusion-eval {code}: {message}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn live() -> Cli {
        Cli {
            dry_run: false,
            runtime_check: false,
            live: true,
            paid_opt_in: true,
            run_count: Some(1),
            budget_nano_usd: Some(1),
            max_model_calls: Some(1),
            comparisons: vec![0],
        }
    }
    #[tokio::test]
    async fn invalid_caps_fail_before_any_runtime_or_credentials() {
        let mut options = live();
        options.paid_opt_in = false;
        assert!(execute(&options, None)
            .await
            .unwrap_err()
            .contains("paid-opt-in"));
        let mut options = live();
        options.max_model_calls = Some(257);
        assert!(execute(&options, None).await.is_err());
        let mut options = live();
        options.budget_nano_usd = Some(1_000_000_001);
        assert!(execute(&options, None).await.is_err());
    }
    #[tokio::test]
    async fn default_offline_enumerates_without_boot() {
        let options = Cli {
            live: false,
            paid_opt_in: false,
            run_count: None,
            budget_nano_usd: None,
            max_model_calls: None,
            comparisons: vec![],
            ..live()
        };
        let (report, success) = execute(&options, None).await.unwrap();
        assert!(success);
        assert_eq!(report["network_calls"], 0);
        assert_eq!(report["comparison_count"], 144);
    }
}
