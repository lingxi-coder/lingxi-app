//! `lingxi-cli plugin eval` — isolated, scored plugin evaluation.
//!
//! The public command surface mirrors Claude Code 2.1.220. Evaluation turns
//! are deliberately launched through the current CLI executable instead of a
//! private orchestrator shortcut: plugin loading, managed deny rules,
//! permission policy, sandboxing, hooks, model selection, and cost accounting
//! therefore use the same production path as an ordinary print-mode turn.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{IsTerminal as _, Write as _};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value as YamlValue};

use crate::exit_codes::{RUNTIME_ERROR, SUCCESS};

const PARTIAL_EXIT: i32 = 2;
const RESULT_SCHEMA_VERSION: u32 = 1;
const DEFAULT_RUNS: u32 = 3;
const DEFAULT_MAX_TURNS: u32 = 20;
const DEFAULT_TIMEOUT_SECONDS: u64 = 600;
const MAX_CASE_FILES: usize = 1_000;
const MAX_STAGE_FILES: usize = 20_000;
const MAX_STAGE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_PROMPT_BYTES: usize = 1_048_576;
const MAX_GRADER_OUTPUT_BYTES: usize = 262_144;

/// The eval directory used when neither `--eval-dir` nor the manifest's
/// `experimental.evals` names one (oracle: "else evals/").
const DEFAULT_EVAL_DIR: &str = "evals";

/// Run eval cases (<eval dir>/**/case.yaml or prompt.md + graders/*.md; the eval
/// dir is evals/ unless --eval-dir or the manifest says otherwise) against a
/// plugin and report scored results. Target is a path, a plugin name, or a
/// `plugin@marketplace` id — installed and skills-dir plugins both resolve (and
/// add a no-plugin baseline arm).
#[derive(Debug, Clone, Args)]
pub struct Cli {
    /// Optional nested eval command.
    #[command(subcommand)]
    pub command: Option<EvalSub>,

    /// Run a no-plugin baseline arm and report the score delta (none |
    /// with-without; default: with-without whenever a plugin resolves — by
    /// name, or from the target path — and none when nothing does; under
    /// with-without, graders marked with-only, incl. `tool_used: Skill`, are a
    /// plugin-fired indicator rather than part of the score).
    #[arg(long, value_name = "mode", value_parser = ["none", "with-without"])]
    pub ablation: Option<String>,

    /// Operator grant for gated tools (Bash, Write, Edit, WebFetch, mcp__*).
    /// Supports Tool(pattern:*) syntax. Managed deny and sandbox policy still
    /// win.
    #[arg(long = "allow-tools", value_name = "tools", num_args = 1..)]
    pub allow_tools: Vec<String>,

    /// Filter cases by name glob.
    #[arg(long = "case", value_name = "glob")]
    pub case_filter: Option<String>,

    /// Directory name (below the plugin) that holds the eval cases; results go
    /// to <plugin>/<dir>/results/ — for an installed-plugin target,
    /// ./<dir>/results/ with this flag, else ./evals/results/ (default dir: the
    /// manifest's experimental.evals value, else evals/)
    //
    // CLI-04 (cc 2.1.238, cc-238.js @235153733 / binary @298656168): NEW in
    // 2.1.238 (0 hits in 2.1.220). WIRED: `resolve_eval_dir` feeds
    // `discover_cases` (case lookup below the plugin root) and `run_init`
    // (where the scaffold is written); the CWD-relative results dir follows the
    // flag verbatim per this help text ("./<dir>/results/ with this flag, else
    // ./evals/results/") and is therefore NOT rerouted by a manifest default.
    #[arg(long = "eval-dir", value_name = "dir")]
    pub eval_dir: Option<String>,

    /// Print JSON to stdout, or write it to an optional path.
    #[arg(
        long,
        value_name = "path",
        num_args = 0..=1,
        default_missing_value = "-"
    )]
    pub json: Option<String>,

    /// Override LLM-grader model.
    #[arg(long = "judge-model", value_name = "model", default_value = "haiku")]
    pub judge_model: String,

    /// Preserve scaffold dirs for debugging.
    #[arg(long = "keep-temp")]
    pub keep_temp: bool,

    /// Optional hard cost ceiling; abort and report partial results if hit
    /// (exit 2). Overrun is bounded to one agent run.
    #[arg(long = "max-cost-usd", value_name = "usd", value_parser = parse_positive_f64)]
    pub max_cost_usd: Option<f64>,

    /// Override model for all cases.
    #[arg(long, value_name = "model")]
    pub model: Option<String>,

    /// Keep the HTML report local only; skip publishing it to claude.ai
    //
    // CLI-04 (cc 2.1.238, cc-238.js @235153173 / binary @298657852): NEW in
    // 2.1.238 (0 hits in 2.1.220). In 2.1.238 publishing became the DEFAULT
    // when the account supports it, so the oracle needs an opt-out; LingXi does
    // not implement the claude.ai report-upload protocol at all (its
    // `--publish-report` twin returns the not-implemented path below), so the
    // report is already local-only and this flag is honoured by construction.
    // Declared so scripts written against 2.1.238 parse, and so the day the
    // upload lands the opt-out is already the operator's to set.
    #[arg(long = "no-publish")]
    pub no_publish: bool,

    /// Explicitly skip case scaffold scripts.
    #[arg(long = "no-scaffold", conflicts_with = "scaffold")]
    pub no_scaffold: bool,

    /// Directory for aggregate-result.json (default:
    /// ./<eval dir>/results/<timestamp>/).
    #[arg(long = "output-dir", value_name = "dir")]
    pub output_dir: Option<PathBuf>,

    /// Also require publishing the report to claude.ai (already the default
    /// when your account supports it); explains why if unavailable.
    #[arg(long = "publish-report")]
    pub publish_report: bool,

    /// Write the self-contained HTML report (scores, prompts, grader verdicts)
    /// to <path> instead of the results dir.
    #[arg(long, value_name = "path")]
    pub report: Option<PathBuf>,

    /// Override per-case runs (default: case.runs ?? 3).
    #[arg(long, value_name = "n", value_parser = parse_positive_u32)]
    pub runs: Option<u32>,

    /// Run each case's scaffold_script (author-supplied shell; off by default).
    #[arg(long, conflicts_with = "no_scaffold")]
    pub scaffold: bool,

    /// Filter cases by tag. Every requested tag must be present.
    #[arg(long, value_name = "tag", num_args = 1.., action = clap::ArgAction::Append)]
    pub tag: Vec<String>,

    /// Exit 1 if any case score is below this threshold.
    #[arg(long, value_name = "0..1", default_value = "1.0", value_parser = parse_threshold)]
    pub threshold: f64,

    /// Log per-message trace events to the debug log (use --debug-file to read
    /// them).
    #[arg(long)]
    pub verbose: bool,

    /// Plugin path, installed name, skills-dir name, or plugin@marketplace id.
    #[arg(value_name = "target")]
    pub target: Option<String>,
}

/// Nested `plugin eval` commands.
#[derive(Debug, Clone, Subcommand)]
pub enum EvalSub {
    /// Author an eval suite under the eval dir (evals/ unless --eval-dir or the
    /// manifest says otherwise) via an interview that sources inputs and
    /// designs graders. Use --bare <name> for a blank single-case template.
    Init(InitArgs),
}

/// `plugin eval init [options] [name]`.
#[derive(Debug, Clone, Args)]
pub struct InitArgs {
    /// Write a blank prompt + criteria template instead of starting the interview.
    #[arg(long)]
    pub bare: bool,

    /// Directory (below the current directory) to write cases into (default:
    /// experimental.evals from the plugin.json in the current directory, else
    /// evals/)
    //
    // CLI-05 (cc 2.1.238, cc-238.js @235153940): NEW in 2.1.238. The oracle
    // resolves `evalDir: c.evalDir ?? a.opts().evalDir`, i.e. this flag falls
    // back to the PARENT `plugin eval --eval-dir` — mirrored in `run`.
    #[arg(long = "eval-dir", value_name = "dir")]
    pub eval_dir: Option<String>,

    /// Run the authoring interview (already the default in a terminal);
    /// requires an interactive terminal
    //
    // CLI-05 (cc 2.1.238, cc-238.js @235153940): NEW in 2.1.238. The oracle's
    // handler `sSw` computes `o = !t.bare && (t.forceInteractive || isTTY)` and
    // refuses with a byte-exact message when `o && !isTTY` — so `--bare` WINS
    // over `-i`, and `-i` only ever adds the refusal. LingXi already runs the
    // interview whenever `--bare` is absent (it does not fall back to a blank
    // template off-TTY — a pre-existing divergence), so this flag contributes
    // exactly the TTY refusal.
    #[arg(short = 'i', long)]
    pub interactive: bool,

    /// Alias for --interactive
    //
    // CLI-05: `.addOption(new bp("--interview","Alias for --interactive")
    // .hideHelp())` — hidden in the oracle's help, OR-ed with `--interactive`
    // (`forceInteractive: c.interactive || c.interview`).
    #[arg(long, hide = true)]
    pub interview: bool,

    /// Eval suite name.
    #[arg(value_name = "name")]
    pub name: Option<String>,
}

/// Versioned normalized eval suite.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalSuite {
    /// Schema version.
    pub schema_version: u32,
    /// Resolved plugin root.
    pub plugin_root: PathBuf,
    /// Cases selected for this run.
    pub cases: Vec<EvalCase>,
}

/// One normalized evaluation case.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalCase {
    /// Stable case name.
    pub name: String,
    /// Agent prompt.
    pub prompt: String,
    /// Optional expected outcome supplied to graders.
    #[serde(default)]
    pub expected_outcome: Option<String>,
    /// Optional tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Default number of runs.
    #[serde(default)]
    pub runs: Option<u32>,
    /// Optional case-specific model.
    #[serde(default)]
    pub model: Option<String>,
    /// Per-agent timeout.
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    /// Per-agent maximum turns.
    #[serde(default)]
    pub max_turns: Option<u32>,
    /// Case-declared tool surface, still narrowed by managed policy.
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    /// Case-specific appended system prompt.
    #[serde(default)]
    pub append_system_prompt: Option<String>,
    /// Optional author-supplied scaffold script.
    #[serde(default)]
    pub scaffold_script: Option<String>,
    /// Normalized graders.
    #[serde(default)]
    pub graders: Vec<GraderDefinition>,
    /// Source case path.
    pub source: PathBuf,
}

/// A free deterministic grader or an LLM rubric grader.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GraderDefinition {
    /// Match the output with a regular expression.
    Regex {
        /// Display name.
        name: String,
        /// Regular expression.
        pattern: String,
        /// Score weight.
        weight: f64,
    },
    /// Require tools to appear in a declared order.
    ToolOrder {
        /// Display name.
        name: String,
        /// Ordered tool names.
        tools: Vec<String>,
        /// Score weight.
        weight: f64,
    },
    /// Require a tool to have been used.
    ToolUsed {
        /// Display name.
        name: String,
        /// Tool name or wildcard.
        tool: String,
        /// Score weight.
        weight: f64,
    },
    /// Require a file to exist in the isolated run directory.
    FileExists {
        /// Display name.
        name: String,
        /// Confined relative path.
        path: PathBuf,
        /// Score weight.
        weight: f64,
    },
    /// Ask a judge model to score a markdown rubric.
    Llm {
        /// Display name.
        name: String,
        /// Rubric.
        rubric: String,
        /// Score weight.
        weight: f64,
    },
    /// Paid comparison grader using a rubric and the no-plugin baseline.
    Baseline {
        /// Display name.
        name: String,
        /// Comparison rubric.
        rubric: String,
        /// Score weight.
        weight: f64,
    },
}

impl GraderDefinition {
    fn name(&self) -> &str {
        match self {
            Self::Regex { name, .. }
            | Self::ToolOrder { name, .. }
            | Self::ToolUsed { name, .. }
            | Self::FileExists { name, .. }
            | Self::Llm { name, .. }
            | Self::Baseline { name, .. } => name,
        }
    }

    fn weight(&self) -> f64 {
        match self {
            Self::Regex { weight, .. }
            | Self::ToolOrder { weight, .. }
            | Self::ToolUsed { weight, .. }
            | Self::FileExists { weight, .. }
            | Self::Llm { weight, .. }
            | Self::Baseline { weight, .. } => *weight,
        }
    }

    fn is_paid(&self) -> bool {
        matches!(self, Self::Llm { .. } | Self::Baseline { .. })
    }
}

/// One grader verdict.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraderResult {
    /// Grader name.
    pub name: String,
    /// Normalized score in `[0, 1]`, or absent when skipped.
    pub score: Option<f64>,
    /// Whether the grader met its full-credit threshold.
    pub passed: bool,
    /// Human-readable reason.
    pub reason: String,
    /// Whether budget policy skipped this grader.
    pub skipped: bool,
}

/// One isolated agent run and its graders.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalRunResult {
    /// Arm name (`with` or `without`).
    pub arm: String,
    /// One-based run number.
    pub run: u32,
    /// Whether the agent subprocess completed successfully.
    pub success: bool,
    /// Whether all scored graders passed.
    pub passed: bool,
    /// Agent output.
    pub output: String,
    /// Agent and grader cost attributed to this run.
    pub cost_usd: f64,
    /// Cost attributable to judge calls.
    pub judge_cost_usd: f64,
    /// Number of agentic turns.
    pub turns: u64,
    /// Wall duration.
    pub duration_seconds: f64,
    /// RFC3339 run start timestamp.
    pub started_at: String,
    /// Trace JSONL path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_path: Option<PathBuf>,
    /// Whether one or more paid graders were skipped.
    pub skipped_paid_graders: bool,
    /// Grader verdicts.
    pub graders: Vec<GraderResult>,
    /// Weighted score.
    pub score: f64,
    /// Optional execution error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Per-arm run collection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalArms {
    /// Runs with the target plugin loaded.
    #[serde(rename = "with")]
    pub with_plugin: Vec<EvalRunResult>,
    /// Runs without the target plugin.
    #[serde(rename = "without", skip_serializing_if = "Option::is_none")]
    pub without_plugin: Option<Vec<EvalRunResult>>,
}

/// Aggregate score block for one case.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalCaseAggregates {
    /// With-plugin score.
    pub score: f64,
    /// With-plugin pass rate.
    pub pass_rate: f64,
    /// Without-plugin score.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score_without: Option<f64>,
    /// Without-plugin pass rate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pass_rate_without: Option<f64>,
    /// With-minus-without score delta.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta: Option<f64>,
}

/// Aggregate result for one case.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalResult {
    /// Case name.
    pub name: String,
    /// Case directory.
    pub dir: PathBuf,
    /// Source case path.
    pub source: PathBuf,
    /// Original prompt markdown.
    pub prompt_markdown: String,
    /// Effective model override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Runs requested per arm.
    pub runs_per_case: u32,
    /// Per-run timeout.
    pub timeout_seconds: u64,
    /// Per-run turn limit.
    pub max_turns: u32,
    /// Tags copied from the case.
    pub tags: Vec<String>,
    /// Normalized graders.
    pub graders: Vec<GraderDefinition>,
    /// Runs grouped by arm.
    pub arms: EvalArms,
    /// Case score summary.
    pub aggregates: EvalCaseAggregates,
}

/// One plugin loaded by the suite.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalSuitePlugin {
    /// Display name or id.
    pub name: String,
    /// Canonical plugin root.
    pub path: PathBuf,
}

/// Public suite configuration block.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalSuiteResult {
    /// Canonical suite root.
    pub root: PathBuf,
    /// Effective ablation mode.
    pub ablation: String,
    /// Named plugin id, when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<String>,
    /// Global model override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_override: Option<String>,
    /// Judge model.
    pub judge_model: String,
    /// Name filter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub case_filter: Option<String>,
    /// Tag filters.
    pub tag_filters: Vec<String>,
    /// Pass threshold.
    pub threshold: f64,
    /// Loaded plugins.
    pub plugins: Vec<EvalSuitePlugin>,
}

/// Top-level aggregate counts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalAggregates {
    /// Total cases.
    pub cases_total: usize,
    /// Cases at or above threshold.
    pub cases_passed: usize,
    /// Mean case score.
    pub overall_score: f64,
    /// Passed-case ratio.
    pub overall_pass_rate: f64,
}

/// Complete machine-readable evaluation result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AggregateResult {
    /// Result schema version.
    pub schema_version: u32,
    /// Compatibility version field carrying this CLI's version.
    pub claude_version: String,
    /// RFC3339 start timestamp.
    pub started_at: String,
    /// Total wall duration.
    pub duration_seconds: f64,
    /// Total measured cost.
    pub cost_usd: f64,
    /// Whether budget or execution failure made this partial.
    pub partial: bool,
    /// Effective suite.
    pub suite: EvalSuiteResult,
    /// Per-case results.
    pub cases: Vec<EvalResult>,
    /// Aggregate counts and score.
    pub aggregates: EvalAggregates,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetKind {
    Path,
    Named,
}

#[derive(Debug)]
struct ResolvedTarget {
    original: String,
    root: PathBuf,
    kind: TargetKind,
    disabled_plugin_ids: Vec<String>,
}

#[derive(Debug)]
struct AgentOutput {
    success: bool,
    text: String,
    cost_usd: f64,
    turns: u64,
    duration_seconds: f64,
    tools_used: Vec<String>,
    trace_path: Option<PathBuf>,
    error: Option<String>,
}

#[derive(Debug)]
struct ParsedAgentStream {
    text: String,
    cost_usd: f64,
    turns: u64,
    duration_seconds: f64,
    tools_used: Vec<String>,
    is_error: bool,
    error_message: Option<String>,
}

struct RunTemp {
    path: PathBuf,
    keep: bool,
}

impl RunTemp {
    fn create(keep: bool) -> Result<Self, String> {
        let path =
            std::env::temp_dir().join(format!("lingxi-plugin-eval-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path)
            .map_err(|error| format!("failed to create isolated eval directory: {error}"))?;
        set_private_dir_permissions(&path)?;
        Ok(Self { path, keep })
    }
}

impl Drop for RunTemp {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

/// Run the eval command.
pub async fn run(cli: &Cli, plugins_dir: &Path, home: &Path, cwd: &Path) -> i32 {
    if let Some(EvalSub::Init(args)) = &cli.command {
        return run_init(args, cli.eval_dir.as_deref(), cwd).await;
    }
    if cli.publish_report {
        eprintln!(
            "lingxi-cli plugin eval: --publish-report requires Anthropic's private claude.ai report upload protocol, which LingXi does not implement"
        );
        return RUNTIME_ERROR;
    }
    match run_evaluation(cli, plugins_dir, home, cwd).await {
        Ok((result, exit)) => {
            if result.cases.is_empty() {
                eprintln!("No eval cases found under {}.", result.suite.root.display());
            }
            if let Err(error) = emit_outputs(cli, &result, cwd) {
                eprintln!("lingxi-cli plugin eval: {error}");
                return PARTIAL_EXIT;
            }
            exit
        }
        Err(error) => {
            eprintln!("lingxi-cli plugin eval: {error}");
            PARTIAL_EXIT
        }
    }
}

async fn run_evaluation(
    cli: &Cli,
    plugins_dir: &Path,
    home: &Path,
    cwd: &Path,
) -> Result<(AggregateResult, i32), String> {
    let started = std::time::Instant::now();
    let started_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let target_text = cli.target.clone().unwrap_or_else(|| ".".to_string());
    let target = resolve_target(&target_text, plugins_dir, home, cwd).await?;
    let eval_dir = resolve_eval_dir(cli.eval_dir.as_deref(), &target.root)?;
    let mut cases = discover_cases(&target.root, &eval_dir)?;
    cases.retain(|case| selected_case(case, cli));

    let ablation = cli.ablation.clone().unwrap_or_else(|| {
        if target.kind == TargetKind::Named {
            "with-without".to_string()
        } else {
            "none".to_string()
        }
    });
    let suite = EvalSuiteResult {
        root: target.root.clone(),
        ablation: ablation.clone(),
        plugin_id: (target.kind == TargetKind::Named).then(|| target.original.clone()),
        model_override: cli.model.clone(),
        judge_model: cli.judge_model.clone(),
        case_filter: cli.case_filter.clone(),
        tag_filters: cli.tag.clone(),
        threshold: cli.threshold,
        plugins: if target.kind == TargetKind::Named
            || target
                .root
                .join(branding::PLUGIN_MANIFEST_DIR)
                .join("plugin.json")
                .is_file()
        {
            vec![EvalSuitePlugin {
                name: target.original.clone(),
                path: target.root.clone(),
            }]
        } else {
            Vec::new()
        },
    };
    if cases.is_empty() {
        return Ok((
            AggregateResult {
                schema_version: RESULT_SCHEMA_VERSION,
                claude_version: env!("CARGO_PKG_VERSION").to_string(),
                started_at,
                duration_seconds: started.elapsed().as_secs_f64(),
                cost_usd: 0.0,
                partial: false,
                suite,
                cases: Vec::new(),
                aggregates: EvalAggregates {
                    cases_total: 0,
                    cases_passed: 0,
                    overall_score: 0.0,
                    overall_pass_rate: 0.0,
                },
            },
            RUNTIME_ERROR,
        ));
    }
    let temp = RunTemp::create(cli.keep_temp)?;
    if cli.keep_temp {
        eprintln!(
            "lingxi-cli plugin eval: preserving run directories at {}",
            temp.path.display()
        );
    }

    let mut total_cost = 0.0;
    let mut partial = false;
    let mut results = Vec::new();
    for case in cases {
        let run_count = cli.runs.or(case.runs).unwrap_or(DEFAULT_RUNS);
        let mut runs = Vec::new();
        for run_number in 1..=run_count {
            if budget_reached(cli.max_cost_usd, total_cost) {
                partial = true;
                break;
            }
            let mut baseline_output = None;
            if ablation == "with-without" {
                let baseline = run_one_arm(
                    cli,
                    &target.root,
                    &target.disabled_plugin_ids,
                    &case,
                    "without",
                    run_number,
                    &temp.path,
                    false,
                    total_cost,
                    None,
                )
                .await;
                total_cost += baseline.cost_usd;
                partial |= !baseline.success || baseline.skipped_paid_graders;
                baseline_output = Some(baseline.output.clone());
                runs.push(baseline);
                if budget_reached(cli.max_cost_usd, total_cost) {
                    partial = true;
                    continue;
                }
            }
            let plugin_run = run_one_arm(
                cli,
                &target.root,
                &target.disabled_plugin_ids,
                &case,
                "with",
                run_number,
                &temp.path,
                true,
                total_cost,
                baseline_output.as_deref(),
            )
            .await;
            total_cost += plugin_run.cost_usd;
            partial |= !plugin_run.success || plugin_run.skipped_paid_graders;
            runs.push(plugin_run);
        }
        let mut with_plugin = Vec::new();
        let mut without_plugin = Vec::new();
        for run in runs {
            if run.arm == "with" {
                with_plugin.push(run);
            } else {
                without_plugin.push(run);
            }
        }
        let score = run_average(&with_plugin);
        let pass_rate = run_pass_rate(&with_plugin);
        let score_without = (ablation == "with-without").then(|| run_average(&without_plugin));
        let pass_rate_without =
            (ablation == "with-without").then(|| run_pass_rate(&without_plugin));
        let case_dir = case.source.parent().unwrap_or(&target.root).to_path_buf();
        results.push(EvalResult {
            name: case.name,
            dir: case_dir,
            source: case.source,
            prompt_markdown: case.prompt,
            model: cli.model.clone().or(case.model),
            runs_per_case: run_count,
            timeout_seconds: case.timeout_seconds.unwrap_or(DEFAULT_TIMEOUT_SECONDS),
            max_turns: case.max_turns.unwrap_or(DEFAULT_MAX_TURNS),
            tags: case.tags,
            graders: case.graders,
            arms: EvalArms {
                with_plugin,
                without_plugin: (ablation == "with-without").then_some(without_plugin),
            },
            aggregates: EvalCaseAggregates {
                score,
                pass_rate,
                score_without,
                pass_rate_without,
                delta: score_without.map(|baseline| score - baseline),
            },
        });
    }
    let score = if results.is_empty() {
        0.0
    } else {
        results
            .iter()
            .map(|case| case.aggregates.score)
            .sum::<f64>()
            / results.len() as f64
    };
    let cases_passed = results
        .iter()
        .filter(|case| case.aggregates.score >= cli.threshold)
        .count();
    let cases_total = results.len();
    let pass_rate = cases_passed as f64 / cases_total as f64;
    let threshold_failed = cases_passed != cases_total;
    let exit = if partial || budget_reached(cli.max_cost_usd, total_cost) {
        PARTIAL_EXIT
    } else if threshold_failed {
        RUNTIME_ERROR
    } else {
        SUCCESS
    };
    Ok((
        AggregateResult {
            schema_version: RESULT_SCHEMA_VERSION,
            claude_version: env!("CARGO_PKG_VERSION").to_string(),
            started_at,
            duration_seconds: started.elapsed().as_secs_f64(),
            cost_usd: total_cost,
            partial,
            suite,
            cases: results,
            aggregates: EvalAggregates {
                cases_total,
                cases_passed,
                overall_score: score,
                overall_pass_rate: pass_rate,
            },
        },
        exit,
    ))
}

#[allow(clippy::too_many_arguments)]
async fn run_one_arm(
    cli: &Cli,
    plugin_root: &Path,
    disabled_plugin_ids: &[String],
    case: &EvalCase,
    arm: &str,
    run_number: u32,
    temp_root: &Path,
    plugin_enabled: bool,
    prior_cost: f64,
    baseline_output: Option<&str>,
) -> EvalRunResult {
    let started_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let run_dir = temp_root
        .join(safe_segment(&case.name))
        .join(format!("{arm}-{run_number}"));
    if let Err(error) = fs::create_dir_all(&run_dir) {
        return failed_run(
            arm,
            run_number,
            format!("failed to create run directory: {error}"),
        );
    }
    if cli.scaffold {
        if let Some(script) = case.scaffold_script.as_deref() {
            eprintln!(
                "lingxi-cli plugin eval: executing scaffold_script from {}",
                case.source.display()
            );
            if let Err(error) = run_scaffold(script, &run_dir).await {
                return failed_run(arm, run_number, error);
            }
            if let Err(error) = validate_tree_confined(&run_dir) {
                return failed_run(arm, run_number, error);
            }
        }
    }

    let model = cli.model.as_deref().or(case.model.as_deref());
    let mut allowed_tools = case.allowed_tools.clone();
    for tool in &cli.allow_tools {
        if !allowed_tools.contains(tool) {
            allowed_tools.push(tool.clone());
        }
    }
    let output = run_agent(
        &case.prompt,
        model,
        &allowed_tools,
        plugin_enabled.then_some(plugin_root),
        disabled_plugin_ids,
        &run_dir,
        case.max_turns.unwrap_or(DEFAULT_MAX_TURNS),
        case.timeout_seconds.unwrap_or(DEFAULT_TIMEOUT_SECONDS),
        cli.verbose,
        case.append_system_prompt.as_deref(),
    )
    .await;
    let mut run_cost = output.cost_usd;
    let mut judge_cost = 0.0;
    let mut grader_results = Vec::new();
    for grader in &case.graders {
        if matches!(grader, GraderDefinition::Baseline { .. }) && baseline_output.is_none() {
            if arm == "without" {
                continue;
            }
            grader_results.push(GraderResult {
                name: grader.name().to_string(),
                score: Some(0.0),
                passed: false,
                reason: "baseline grader requires --ablation with-without".to_string(),
                skipped: false,
            });
            continue;
        }
        if grader.is_paid() && budget_reached(cli.max_cost_usd, prior_cost + run_cost) {
            grader_results.push(GraderResult {
                name: grader.name().to_string(),
                score: None,
                passed: false,
                reason: "skipped after cost ceiling was reached".to_string(),
                skipped: true,
            });
            continue;
        }
        let (result, grader_cost) = grade_output(
            grader,
            &case.prompt,
            case.expected_outcome.as_deref(),
            &output.text,
            &output.tools_used,
            &cli.judge_model,
            &run_dir,
            baseline_output,
        )
        .await;
        run_cost += grader_cost;
        judge_cost += grader_cost;
        grader_results.push(result);
    }
    let score = weighted_score(&grader_results, &case.graders, output.success);
    let skipped_paid_graders = grader_results.iter().any(|result| result.skipped);
    EvalRunResult {
        arm: arm.to_string(),
        run: run_number,
        success: output.success,
        passed: output.success && score >= 1.0,
        output: output.text,
        cost_usd: run_cost,
        judge_cost_usd: judge_cost,
        turns: output.turns,
        duration_seconds: output.duration_seconds,
        started_at,
        trace_path: output.trace_path,
        skipped_paid_graders,
        graders: grader_results,
        score,
        error: output.error,
    }
}

fn failed_run(arm: &str, run: u32, error: String) -> EvalRunResult {
    EvalRunResult {
        arm: arm.to_string(),
        run,
        success: false,
        passed: false,
        output: String::new(),
        cost_usd: 0.0,
        judge_cost_usd: 0.0,
        turns: 0,
        duration_seconds: 0.0,
        started_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        trace_path: None,
        skipped_paid_graders: false,
        graders: Vec::new(),
        score: 0.0,
        error: Some(error),
    }
}

async fn run_agent(
    prompt: &str,
    model: Option<&str>,
    allow_tools: &[String],
    plugin_root: Option<&Path>,
    disabled_plugin_ids: &[String],
    cwd: &Path,
    max_turns: u32,
    timeout_seconds: u64,
    verbose: bool,
    append_system_prompt: Option<&str>,
) -> AgentOutput {
    // `--settings {"enabledPlugins": ...}` is retained below as
    // defense-in-depth, but the desktop composition root intentionally reads
    // ambient plugin enablement from the persisted user/project files. Bind
    // every eval child (including judge runs) to a fresh empty cache so a
    // globally enabled copy of the target cannot leak into the baseline arm.
    // The with-plugin arm still receives the target exclusively through the
    // explicit `--plugin-dir` path.
    let isolated_plugin_cache = match RunTemp::create(false) {
        Ok(cache) => cache,
        Err(error) => {
            return AgentOutput {
                success: false,
                text: String::new(),
                cost_usd: 0.0,
                turns: 0,
                duration_seconds: 0.0,
                tools_used: Vec::new(),
                trace_path: None,
                error: Some(format!(
                    "failed to create isolated eval plugin cache: {error}"
                )),
            };
        }
    };
    let executable = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            return AgentOutput {
                success: false,
                text: String::new(),
                cost_usd: 0.0,
                turns: 0,
                duration_seconds: 0.0,
                tools_used: Vec::new(),
                trace_path: None,
                error: Some(format!("failed to resolve current executable: {error}")),
            };
        }
    };
    let mut command = tokio::process::Command::new(executable);
    command
        .env("LINGXI_PLUGIN_CACHE_DIR", &isolated_plugin_cache.path)
        .arg("--print")
        .arg("--output-format")
        .arg("stream-json")
        .arg("--verbose")
        .arg("--no-session-persistence")
        .arg("--cwd")
        .arg(cwd)
        .arg("--max-turns")
        .arg(max_turns.to_string());
    if let Some(model) = model {
        command.arg("--model").arg(model);
    }
    if let Some(root) = plugin_root {
        command.arg("--plugin-dir").arg(root);
    }
    if !disabled_plugin_ids.is_empty() {
        let disabled = disabled_plugin_ids
            .iter()
            .map(|id| (id.clone(), serde_json::Value::Bool(false)))
            .collect::<serde_json::Map<_, _>>();
        command
            .arg("--settings")
            .arg(serde_json::json!({ "enabledPlugins": disabled }).to_string());
    }
    if let Some(system_prompt) = append_system_prompt {
        command.arg("--append-system-prompt").arg(system_prompt);
    }
    if !allow_tools.is_empty() {
        command.arg("--allowedTools").args(allow_tools);
    }
    command
        .arg("--")
        .arg(prompt)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let result = tokio::time::timeout(Duration::from_secs(timeout_seconds), command.output()).await;
    let output = match result {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            return AgentOutput {
                success: false,
                text: String::new(),
                cost_usd: 0.0,
                turns: 0,
                duration_seconds: 0.0,
                tools_used: Vec::new(),
                trace_path: None,
                error: Some(format!("failed to start eval agent: {error}")),
            };
        }
        Err(_) => {
            return AgentOutput {
                success: false,
                text: String::new(),
                cost_usd: 0.0,
                turns: 0,
                duration_seconds: timeout_seconds as f64,
                tools_used: Vec::new(),
                trace_path: None,
                error: Some(format!("eval agent timed out after {timeout_seconds}s")),
            };
        }
    };
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if verbose && !stderr.is_empty() {
        eprintln!("{stderr}");
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let trace_path = cwd.join("trace.jsonl");
    let persisted_trace = atomic_write(&trace_path, stdout.as_bytes()).is_ok();
    let parsed = match parse_agent_stream_output(&stdout) {
        Ok(parsed) => parsed,
        Err(protocol_error) => {
            let error = if stderr.is_empty() {
                protocol_error
            } else {
                format!("{protocol_error}; stderr: {stderr}")
            };
            return AgentOutput {
                success: false,
                text: String::new(),
                cost_usd: 0.0,
                turns: 0,
                duration_seconds: 0.0,
                tools_used: Vec::new(),
                trace_path: persisted_trace.then_some(trace_path),
                error: Some(error),
            };
        }
    };
    let success = output.status.success() && !parsed.is_error;
    AgentOutput {
        success,
        text: parsed.text,
        cost_usd: parsed.cost_usd,
        turns: parsed.turns,
        duration_seconds: parsed.duration_seconds,
        tools_used: parsed.tools_used,
        trace_path: persisted_trace.then_some(trace_path),
        error: (!success).then(|| {
            if stderr.is_empty() {
                parsed
                    .error_message
                    .unwrap_or_else(|| format!("eval agent exited with {}", output.status))
            } else {
                stderr
            }
        }),
    }
}

fn parse_agent_stream_output(stdout: &str) -> Result<ParsedAgentStream, String> {
    let mut frames = Vec::new();
    for (index, line) in stdout.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let frame = serde_json::from_str::<serde_json::Value>(line).map_err(|error| {
            format!(
                "invalid stream-json frame on line {}: {error}",
                index.saturating_add(1)
            )
        })?;
        frames.push(frame);
    }

    let result_indexes = frames
        .iter()
        .enumerate()
        .filter_map(|(index, frame)| {
            (frame.get("type").and_then(serde_json::Value::as_str) == Some("result"))
                .then_some(index)
        })
        .collect::<Vec<_>>();
    if result_indexes.len() != 1 {
        return Err(format!(
            "eval agent stream must contain exactly one terminal result frame; found {}",
            result_indexes.len()
        ));
    }
    let result_index = result_indexes[0];
    if result_index + 1 != frames.len() {
        return Err("eval agent result frame must be terminal".to_string());
    }
    let envelope = &frames[result_index];
    let is_error = envelope
        .get("is_error")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| "eval agent result is missing boolean is_error".to_string())?;
    let cost_usd = envelope
        .get("total_cost_usd")
        .and_then(serde_json::Value::as_f64)
        .filter(|cost| cost.is_finite() && *cost >= 0.0)
        .ok_or_else(|| {
            "eval agent result is missing a finite non-negative total_cost_usd".to_string()
        })?;
    let turns = envelope
        .get("num_turns")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "eval agent result is missing integer num_turns".to_string())?;
    let duration_ms = envelope
        .get("duration_ms")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "eval agent result is missing integer duration_ms".to_string())?;
    let text = envelope
        .get("result")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            is_error.then(|| {
                envelope
                    .get("errors")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(serde_json::Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n")
            })
        })
        .ok_or_else(|| "eval agent result is missing string result".to_string())?;
    let error_message = is_error.then(|| {
        if text.is_empty() {
            "eval agent reported an error".to_string()
        } else {
            text.clone()
        }
    });
    let mut tools_used = Vec::new();
    for frame in &frames {
        collect_tool_names(frame, &mut tools_used);
    }

    Ok(ParsedAgentStream {
        text,
        cost_usd,
        turns,
        duration_seconds: duration_ms as f64 / 1_000.0,
        tools_used,
        is_error,
        error_message,
    })
}

async fn grade_output(
    grader: &GraderDefinition,
    case_prompt: &str,
    expected_outcome: Option<&str>,
    output: &str,
    tools_used: &[String],
    judge_model: &str,
    cwd: &Path,
    baseline_output: Option<&str>,
) -> (GraderResult, f64) {
    match grader {
        GraderDefinition::Regex { name, pattern, .. } => {
            let matched = regex::Regex::new(pattern)
                .map(|regex| regex.is_match(output))
                .unwrap_or(false);
            (
                GraderResult {
                    name: name.clone(),
                    score: Some(if matched { 1.0 } else { 0.0 }),
                    passed: matched,
                    reason: if matched {
                        format!("output matched /{pattern}/")
                    } else {
                        format!("output did not match /{pattern}/")
                    },
                    skipped: false,
                },
                0.0,
            )
        }
        GraderDefinition::ToolOrder { name, tools, .. } => {
            let mut cursor = 0;
            let matched = tools.iter().all(|wanted| {
                let found = tools_used[cursor..]
                    .iter()
                    .position(|actual| tool_matches(wanted, actual));
                if let Some(offset) = found {
                    cursor += offset + 1;
                    true
                } else {
                    false
                }
            });
            (
                GraderResult {
                    name: name.clone(),
                    score: Some(if matched { 1.0 } else { 0.0 }),
                    passed: matched,
                    reason: if matched {
                        format!("tools appeared in order: {}", tools.join(", "))
                    } else {
                        format!(
                            "required tool order {} was not observed in {}",
                            tools.join(", "),
                            tools_used.join(", ")
                        )
                    },
                    skipped: false,
                },
                0.0,
            )
        }
        GraderDefinition::ToolUsed { name, tool, .. } => {
            let matched = tools_used.iter().any(|actual| tool_matches(tool, actual));
            (
                GraderResult {
                    name: name.clone(),
                    score: Some(if matched { 1.0 } else { 0.0 }),
                    passed: matched,
                    reason: if matched {
                        format!("tool {tool:?} was used")
                    } else {
                        format!("tool {tool:?} was not used")
                    },
                    skipped: false,
                },
                0.0,
            )
        }
        GraderDefinition::FileExists { name, path, .. } => {
            let exists = confined_output_path(cwd, path)
                .and_then(|path| fs::metadata(path).map_err(|error| error.to_string()))
                .map(|metadata| metadata.is_file())
                .unwrap_or(false);
            (
                GraderResult {
                    name: name.clone(),
                    score: Some(if exists { 1.0 } else { 0.0 }),
                    passed: exists,
                    reason: if exists {
                        format!("{} exists", path.display())
                    } else {
                        format!("{} does not exist", path.display())
                    },
                    skipped: false,
                },
                0.0,
            )
        }
        GraderDefinition::Llm { name, rubric, .. }
        | GraderDefinition::Baseline { name, rubric, .. } => {
            let bounded_output = truncate_utf8(output, MAX_GRADER_OUTPUT_BYTES);
            let judge_prompt = if matches!(grader, GraderDefinition::Baseline { .. }) {
                let bounded_baseline =
                    truncate_utf8(baseline_output.unwrap_or_default(), MAX_GRADER_OUTPUT_BYTES);
                format!(
                    "Compare the candidate response produced with the plugin to the baseline \
                     response produced without it. Score how well the candidate improves on or \
                     preserves the baseline under the rubric. Return only JSON with fields score \
                     (number from 0 to 1) and reason (string).\n\n\
                     <case_prompt>\n{case_prompt}\n</case_prompt>\n\
                     <expected_outcome>\n{}\n</expected_outcome>\n\
                     <rubric>\n{rubric}\n</rubric>\n\
                     <baseline_response>\n{bounded_baseline}\n</baseline_response>\n\
                     <candidate_response>\n{bounded_output}\n</candidate_response>",
                    expected_outcome.unwrap_or_default()
                )
            } else {
                format!(
                    "Score the candidate response against the rubric. Return only JSON with \
                     fields score (number from 0 to 1) and reason (string).\n\n\
                     <case_prompt>\n{case_prompt}\n</case_prompt>\n\
                     <expected_outcome>\n{}\n</expected_outcome>\n\
                     <rubric>\n{rubric}\n</rubric>\n\
                     <candidate_response>\n{bounded_output}\n</candidate_response>",
                    expected_outcome.unwrap_or_default()
                )
            };
            let judge_dir = cwd.join("judges").join(safe_segment(name));
            if let Err(error) = fs::create_dir_all(&judge_dir) {
                return (
                    GraderResult {
                        name: name.clone(),
                        score: Some(0.0),
                        passed: false,
                        reason: format!("failed to create judge directory: {error}"),
                        skipped: false,
                    },
                    0.0,
                );
            }
            let judged = run_agent(
                &judge_prompt,
                Some(judge_model),
                &[],
                None,
                &[],
                &judge_dir,
                1,
                DEFAULT_TIMEOUT_SECONDS,
                false,
                None,
            )
            .await;
            let parsed = parse_judge_result(&judged.text);
            let (score, reason) = parsed.unwrap_or_else(|| {
                (
                    0.0,
                    judged
                        .error
                        .unwrap_or_else(|| "judge did not return valid score JSON".to_string()),
                )
            });
            (
                GraderResult {
                    name: name.clone(),
                    score: Some(score),
                    passed: score >= 1.0,
                    reason,
                    skipped: false,
                },
                judged.cost_usd,
            )
        }
    }
}

fn parse_judge_result(raw: &str) -> Option<(f64, String)> {
    let trimmed = raw.trim();
    let candidates = [
        trimmed,
        trimmed
            .strip_prefix("```json")
            .and_then(|value| value.strip_suffix("```"))
            .map(str::trim)
            .unwrap_or(""),
        trimmed
            .strip_prefix("```")
            .and_then(|value| value.strip_suffix("```"))
            .map(str::trim)
            .unwrap_or(""),
    ];
    for candidate in candidates {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(candidate) else {
            continue;
        };
        let score = value.get("score")?.as_f64()?.clamp(0.0, 1.0);
        let reason = value
            .get("reason")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        return Some((score, reason));
    }
    None
}

fn collect_tool_names(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(object) => {
            if object.get("type").and_then(serde_json::Value::as_str) == Some("tool_use") {
                if let Some(name) = object.get("name").and_then(serde_json::Value::as_str) {
                    out.push(name.to_string());
                }
            }
            for child in object.values() {
                collect_tool_names(child, out);
            }
        }
        serde_json::Value::Array(array) => {
            for child in array {
                collect_tool_names(child, out);
            }
        }
        _ => {}
    }
}

fn tool_matches(pattern: &str, actual: &str) -> bool {
    pattern == actual || glob_matches(pattern, actual)
}

fn confined_output_path(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(format!(
            "file grader path {} escapes its run",
            relative.display()
        ));
    }
    let path = root.join(relative);
    let parent = path.parent().unwrap_or(root);
    let canonical_root = fs::canonicalize(root)
        .map_err(|error| format!("failed to resolve {}: {error}", root.display()))?;
    let canonical_parent = fs::canonicalize(parent)
        .map_err(|error| format!("failed to resolve {}: {error}", parent.display()))?;
    if !canonical_parent.starts_with(canonical_root) {
        return Err(format!(
            "file grader path {} escapes its run",
            relative.display()
        ));
    }
    reject_symlink(&path)?;
    Ok(path)
}

fn weighted_score(
    results: &[GraderResult],
    definitions: &[GraderDefinition],
    agent_success: bool,
) -> f64 {
    if results.is_empty() {
        return if agent_success { 1.0 } else { 0.0 };
    }
    let weights: BTreeMap<&str, f64> = definitions
        .iter()
        .map(|grader| (grader.name(), grader.weight()))
        .collect();
    let mut total = 0.0;
    let mut weight = 0.0;
    for result in results {
        let Some(score) = result.score else {
            continue;
        };
        let item_weight = weights.get(result.name.as_str()).copied().unwrap_or(1.0);
        total += score * item_weight;
        weight += item_weight;
    }
    if weight == 0.0 {
        0.0
    } else {
        total / weight
    }
}

fn run_average(runs: &[EvalRunResult]) -> f64 {
    if runs.is_empty() {
        0.0
    } else {
        runs.iter().map(|run| run.score).sum::<f64>() / runs.len() as f64
    }
}

fn run_pass_rate(runs: &[EvalRunResult]) -> f64 {
    if runs.is_empty() {
        0.0
    } else {
        runs.iter().filter(|run| run.passed).count() as f64 / runs.len() as f64
    }
}

fn budget_reached(max: Option<f64>, cost: f64) -> bool {
    max.is_some_and(|limit| cost >= limit)
}

fn scaffold_sandbox_policy(cwd: &Path, deny_read: &[PathBuf]) -> serde_json::Value {
    serde_json::json!({
        "enabled": true,
        "failIfUnavailable": true,
        "allowUnsandboxedCommands": false,
        "network": {
            "allowedDomains": []
        },
        "filesystem": {
            "allowWrite": [cwd.to_string_lossy()],
            "denyRead": deny_read,
            "allowRead": [cwd.to_string_lossy()]
        }
    })
}

fn scaffold_denied_read_paths(cwd: &Path) -> Vec<PathBuf> {
    let mut denied = Vec::new();
    if let Some(home) = dirs::home_dir() {
        denied.extend(
            [
                ".ssh",
                ".aws",
                ".azure",
                ".kube",
                ".docker",
                ".config/gcloud",
                ".codex",
                ".claude",
                "Library/Keychains",
            ]
            .into_iter()
            .map(|relative| home.join(relative))
            .filter(|path| !path.starts_with(cwd)),
        );
        let global_config = home.join(branding::GLOBAL_CONFIG_FILE);
        if !global_config.starts_with(cwd) {
            denied.push(global_config);
        }
    }
    let lingxi_home = crate::run::lingxi_home_dir();
    if !lingxi_home.starts_with(cwd) && !denied.contains(&lingxi_home) {
        denied.push(lingxi_home);
    }
    denied
}

async fn run_scaffold(script: &str, cwd: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        let _ = (script, cwd);
        return Err(
            "scaffold scripts require a supported host sandbox; Windows is not supported"
                .to_string(),
        );
    }

    #[cfg(not(windows))]
    let canonical_cwd = fs::canonicalize(cwd)
        .map_err(|error| format!("failed to resolve scaffold root {}: {error}", cwd.display()))?;
    #[cfg(not(windows))]
    let platform = platform_posix::sandbox::host_platform().ok_or_else(|| {
        "scaffold scripts require a supported host sandbox; this platform is unavailable"
            .to_string()
    })?;
    #[cfg(not(windows))]
    let deny_read = scaffold_denied_read_paths(&canonical_cwd);
    #[cfg(not(windows))]
    let policy = serde_json::from_value(scaffold_sandbox_policy(&canonical_cwd, &deny_read))
        .map_err(|error| format!("failed to construct scaffold sandbox policy: {error}"))?;
    #[cfg(not(windows))]
    let runner = engine_desktop::new_live_sandbox_runner();
    #[cfg(not(windows))]
    let wrapped = runner
        .wrap(
            script,
            &policy,
            platform,
            Some("/bin/sh"),
            Some(&canonical_cwd),
        )
        .await
        .map_err(|error| format!("failed to prepare scaffold sandbox: {error}"))?;
    #[cfg(not(windows))]
    let mut command = {
        let mut command = tokio::process::Command::new("/bin/sh");
        command.args(["-c", &wrapped]);
        command
    };
    #[cfg(not(windows))]
    command
        .current_dir(&canonical_cwd)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(not(windows))]
    for key in ["PATH", "LANG", "LC_ALL"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    #[cfg(not(windows))]
    command
        .env("HOME", &canonical_cwd)
        .env("TMPDIR", &canonical_cwd);
    #[cfg(not(windows))]
    let output_result = tokio::time::timeout(
        Duration::from_secs(DEFAULT_TIMEOUT_SECONDS),
        command.output(),
    )
    .await;
    #[cfg(not(windows))]
    runner.cleanup_after_command().await;
    #[cfg(not(windows))]
    runner.reset().await;
    #[cfg(not(windows))]
    let output = output_result
        .map_err(|_| "scaffold script timed out".to_string())?
        .map_err(|error| format!("failed to run scaffold script: {error}"))?;
    #[cfg(not(windows))]
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "scaffold script failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn validate_tree_confined(root: &Path) -> Result<(), String> {
    let canonical_root = fs::canonicalize(root).map_err(|error| {
        format!(
            "failed to resolve {} after scaffold: {error}",
            root.display()
        )
    })?;
    let mut pending = vec![root.to_path_buf()];
    let mut seen = 0usize;
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).map_err(|error| {
            format!(
                "failed to inspect scaffold output {}: {error}",
                dir.display()
            )
        })? {
            let entry =
                entry.map_err(|error| format!("failed to inspect scaffold output: {error}"))?;
            seen += 1;
            if seen > MAX_CASE_FILES {
                return Err(format!(
                    "scaffold output exceeded the {MAX_CASE_FILES}-entry safety limit"
                ));
            }
            let path = entry.path();
            let ty = entry
                .file_type()
                .map_err(|error| format!("failed to inspect {}: {error}", path.display()))?;
            if ty.is_symlink() {
                return Err(format!(
                    "scaffold created a symlink or reparse point: {}",
                    path.display()
                ));
            }
            let canonical = fs::canonicalize(&path)
                .map_err(|error| format!("failed to resolve {}: {error}", path.display()))?;
            if !canonical.starts_with(&canonical_root) {
                return Err(format!(
                    "scaffold output escaped its run: {}",
                    path.display()
                ));
            }
            if ty.is_dir() {
                pending.push(path);
            }
        }
    }
    Ok(())
}

async fn resolve_target(
    target: &str,
    plugins_dir: &Path,
    home: &Path,
    cwd: &Path,
) -> Result<ResolvedTarget, String> {
    let raw_path = Path::new(target);
    let candidate = if raw_path.is_absolute() {
        raw_path.to_path_buf()
    } else {
        cwd.join(raw_path)
    };
    if candidate.exists() || looks_like_path(target) {
        let root = canonical_plugin_root(&candidate)?;
        let disabled_plugin_ids = installed_ids_for_root(plugins_dir, &root, home)?;
        return Ok(ResolvedTarget {
            original: target.to_string(),
            root,
            kind: TargetKind::Path,
            disabled_plugin_ids,
        });
    }

    if let Some(path) = resolve_installed_target(target, plugins_dir)? {
        let root = canonical_plugin_root(&path)?;
        let mut disabled_plugin_ids = installed_ids_for_root(plugins_dir, &root, home)?;
        if target.contains('@') {
            disabled_plugin_ids.push(target.to_string());
            disabled_plugin_ids.sort();
            disabled_plugin_ids.dedup();
        }
        return Ok(ResolvedTarget {
            original: target.to_string(),
            root,
            kind: TargetKind::Named,
            disabled_plugin_ids,
        });
    }
    let skills_name = target.strip_suffix("@skills-dir").unwrap_or(target);
    let skills_path = home.join("skills").join(skills_name);
    if skills_path.is_dir() {
        let root = canonical_plugin_root(&skills_path)?;
        let mut disabled_plugin_ids = installed_ids_for_root(plugins_dir, &root, home)?;
        disabled_plugin_ids.push(format!("{skills_name}@skills-dir"));
        disabled_plugin_ids.sort();
        disabled_plugin_ids.dedup();
        return Ok(ResolvedTarget {
            original: target.to_string(),
            root,
            kind: TargetKind::Named,
            disabled_plugin_ids,
        });
    }

    let matches: Vec<PathBuf> = plugin::discover_recorded_plugins(plugins_dir)
        .await
        .into_iter()
        .filter(|(_, manifest, _)| manifest.name == target)
        .map(|(_, _, path)| path)
        .collect();
    match matches.as_slice() {
        [path] => {
            let root = canonical_plugin_root(path)?;
            let disabled_plugin_ids = installed_ids_for_root(plugins_dir, &root, home)?;
            Ok(ResolvedTarget {
                original: target.to_string(),
                root,
                kind: TargetKind::Named,
                disabled_plugin_ids,
            })
        }
        [] => Err(format!("plugin target {target:?} was not found")),
        _ => Err(format!(
            "plugin name {target:?} is ambiguous; use plugin@marketplace"
        )),
    }
}

fn installed_ids_for_root(
    plugins_dir: &Path,
    root: &Path,
    home: &Path,
) -> Result<Vec<String>, String> {
    let mut ids = Vec::new();
    let path = plugins_dir.join("installed_plugins.json");
    if let Ok(raw) = fs::read_to_string(&path) {
        let value: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;
        if let Some(records) = value.get("plugins").and_then(serde_json::Value::as_object) {
            for (id, record) in records {
                if let Some(items) = record.as_array() {
                    if items.iter().any(|item| {
                        item.get("installPath")
                            .and_then(serde_json::Value::as_str)
                            .and_then(|path| fs::canonicalize(path).ok())
                            .as_deref()
                            == Some(root)
                    }) {
                        ids.push(id.clone());
                    }
                }
                if let Some(nested) = record.as_object() {
                    for (name, details) in nested {
                        let version = details
                            .get("version")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("unknown");
                        let legacy = plugins_dir.join("cache").join(id).join(name).join(version);
                        if fs::canonicalize(legacy).ok().as_deref() == Some(root) {
                            ids.push(format!("{name}@{id}"));
                        }
                    }
                }
            }
        }
    }
    if let Ok(relative) = root.strip_prefix(home.join("skills")) {
        if relative.components().count() == 1 {
            ids.push(format!("{}@skills-dir", relative.to_string_lossy()));
        }
    }
    ids.sort();
    ids.dedup();
    Ok(ids)
}

fn resolve_installed_target(target: &str, plugins_dir: &Path) -> Result<Option<PathBuf>, String> {
    let path = plugins_dir.join("installed_plugins.json");
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("failed to read {}: {error}", path.display())),
    };
    let value: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;
    let Some(records) = value.get("plugins").and_then(serde_json::Value::as_object) else {
        return Ok(None);
    };
    let mut paths = Vec::new();
    for (id, value) in records {
        let matches = id == target
            || (!target.contains('@')
                && id.split_once('@').is_some_and(|(name, _)| name == target));
        if matches {
            if let Some(items) = value.as_array() {
                paths.extend(items.iter().filter_map(|item| {
                    item.get("installPath")
                        .and_then(serde_json::Value::as_str)
                        .map(PathBuf::from)
                }));
            }
        }
        if let Some(nested) = value.as_object() {
            for (name, record) in nested {
                let legacy_id = format!("{name}@{id}");
                if (legacy_id == target || (!target.contains('@') && name == target))
                    && record.is_object()
                {
                    let version = record
                        .get("version")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("unknown");
                    paths.push(plugins_dir.join("cache").join(id).join(name).join(version));
                }
            }
        }
    }
    paths.sort();
    paths.dedup();
    match paths.as_slice() {
        [] => Ok(None),
        [path] => Ok(Some(path.clone())),
        _ => Err(format!(
            "plugin target {target:?} resolves to multiple installations"
        )),
    }
}

fn canonical_plugin_root(path: &Path) -> Result<PathBuf, String> {
    let canonical = fs::canonicalize(path)
        .map_err(|error| format!("failed to resolve {}: {error}", path.display()))?;
    if !canonical.is_dir() {
        return Err(format!("{} is not a directory", canonical.display()));
    }
    Ok(canonical)
}

fn looks_like_path(target: &str) -> bool {
    target == "."
        || target == ".."
        || target.starts_with("./")
        || target.starts_with("../")
        || target.contains(std::path::MAIN_SEPARATOR)
}

fn discover_cases(plugin_root: &Path, eval_dir: &str) -> Result<Vec<EvalCase>, String> {
    let evals_root = plugin_root.join(eval_dir);
    if !evals_root.is_dir() {
        return Ok(Vec::new());
    }
    reject_symlink(&evals_root)?;
    let mut files = Vec::new();
    collect_eval_files(&evals_root, &mut files)?;
    files.sort();

    let mut cases = Vec::new();
    let mut prompt_dirs = Vec::new();
    for path in files {
        match path.file_name().and_then(|name| name.to_str()) {
            Some("case.yaml" | "case.yml") => cases.push(parse_case_yaml(&path)?),
            Some("prompt.md") => {
                prompt_dirs.push(path.parent().unwrap_or(&evals_root).to_path_buf())
            }
            _ => {}
        }
    }
    for dir in prompt_dirs {
        if dir.join("case.yaml").exists() || dir.join("case.yml").exists() {
            continue;
        }
        cases.push(parse_prompt_case(&dir)?);
    }
    cases.sort_by(|left, right| left.name.cmp(&right.name));
    let mut names = std::collections::BTreeSet::new();
    for case in &cases {
        if !names.insert(case.name.clone()) {
            return Err(format!("duplicate eval case name {:?}", case.name));
        }
    }
    Ok(cases)
}

fn collect_eval_files(root: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    if files.len() >= MAX_CASE_FILES {
        return Err(format!(
            "eval discovery exceeded the {MAX_CASE_FILES}-file safety limit"
        ));
    }
    let entries = fs::read_dir(root)
        .map_err(|error| format!("failed to read {}: {error}", root.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("failed to read eval entry: {error}"))?;
        let path = entry.path();
        let ty = entry
            .file_type()
            .map_err(|error| format!("failed to inspect {}: {error}", path.display()))?;
        if ty.is_symlink() {
            return Err(format!(
                "symlinks are not allowed in eval suites: {}",
                path.display()
            ));
        }
        if ty.is_dir() {
            collect_eval_files(&path, files)?;
        } else if ty.is_file() {
            files.push(path);
            if files.len() >= MAX_CASE_FILES {
                return Err(format!(
                    "eval discovery exceeded the {MAX_CASE_FILES}-file safety limit"
                ));
            }
        }
    }
    Ok(())
}

fn parse_case_yaml(path: &Path) -> Result<EvalCase, String> {
    let raw = read_bounded(path, MAX_PROMPT_BYTES)?;
    let value: YamlValue = serde_yaml::from_str(&raw)
        .map_err(|error| format!("invalid {}: {error}", path.display()))?;
    let map = value
        .as_mapping()
        .ok_or_else(|| format!("{} must contain a YAML object", path.display()))?;
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let name = yaml_string(map, "name").unwrap_or_else(|| {
        dir.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    });
    let (prompt_frontmatter, prompt_file_body) = match yaml_string(map, "prompt") {
        Some(prompt) => (Mapping::new(), prompt),
        None => {
            let prompt_path = dir.join("prompt.md");
            let raw = read_bounded(&prompt_path, MAX_PROMPT_BYTES).map_err(|_| {
                format!("{} must define prompt or provide prompt.md", path.display())
            })?;
            parse_markdown_frontmatter(&raw, &prompt_path)?
        }
    };
    let prompt = prompt_file_body;
    if prompt.trim().is_empty() {
        return Err(format!("eval case {name:?} has an empty prompt"));
    }
    let graders = if let Some(value) = yaml_value(map, "graders") {
        parse_yaml_graders(value, dir)?
    } else {
        discover_markdown_graders(dir)?
    };
    Ok(EvalCase {
        name,
        prompt,
        expected_outcome: yaml_string(map, "expected_outcome")
            .or_else(|| yaml_string(map, "expectedOutcome"))
            .or_else(|| yaml_string(&prompt_frontmatter, "expected_outcome")),
        tags: yaml_strings(map, "tags")?,
        runs: yaml_u64(map, "runs")
            .map(|value| u32::try_from(value).map_err(|_| "runs is too large".to_string()))
            .transpose()?,
        model: yaml_string(map, "model"),
        timeout_seconds: yaml_u64(map, "timeout_seconds")
            .or_else(|| yaml_u64(map, "timeoutSeconds")),
        max_turns: yaml_u64(map, "max_turns")
            .or_else(|| yaml_u64(map, "maxTurns"))
            .or_else(|| yaml_u64(&prompt_frontmatter, "max_turns"))
            .map(|value| u32::try_from(value).map_err(|_| "max_turns is too large".to_string()))
            .transpose()?,
        allowed_tools: {
            let from_case = yaml_strings(map, "allowed_tools")?;
            if from_case.is_empty() {
                yaml_strings(&prompt_frontmatter, "allowed_tools")?
            } else {
                from_case
            }
        },
        append_system_prompt: yaml_string(map, "append_system_prompt")
            .or_else(|| yaml_string(map, "appendSystemPrompt"))
            .or_else(|| yaml_string(&prompt_frontmatter, "append_system_prompt")),
        scaffold_script: yaml_string(map, "scaffold_script")
            .or_else(|| yaml_string(map, "scaffoldScript")),
        graders,
        source: path.to_path_buf(),
    })
}

fn parse_prompt_case(dir: &Path) -> Result<EvalCase, String> {
    let prompt_path = dir.join("prompt.md");
    let raw = read_bounded(&prompt_path, MAX_PROMPT_BYTES)?;
    let (frontmatter, prompt) = parse_markdown_frontmatter(&raw, &prompt_path)?;
    if prompt.trim().is_empty() {
        return Err(format!("{} is empty", prompt_path.display()));
    }
    Ok(EvalCase {
        name: dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        prompt,
        expected_outcome: yaml_string(&frontmatter, "expected_outcome"),
        tags: yaml_strings(&frontmatter, "tags")?,
        runs: yaml_u64(&frontmatter, "runs")
            .map(|value| u32::try_from(value).map_err(|_| "runs is too large".to_string()))
            .transpose()?,
        model: yaml_string(&frontmatter, "model"),
        timeout_seconds: yaml_u64(&frontmatter, "timeout_seconds"),
        max_turns: yaml_u64(&frontmatter, "max_turns")
            .map(|value| u32::try_from(value).map_err(|_| "max_turns is too large".to_string()))
            .transpose()?,
        allowed_tools: yaml_strings(&frontmatter, "allowed_tools")?,
        append_system_prompt: yaml_string(&frontmatter, "append_system_prompt"),
        scaffold_script: yaml_string(&frontmatter, "scaffold_script"),
        graders: discover_markdown_graders(dir)?,
        source: prompt_path,
    })
}

fn discover_markdown_graders(case_dir: &Path) -> Result<Vec<GraderDefinition>, String> {
    let graders_dir = case_dir.join("graders");
    if !graders_dir.is_dir() {
        return Ok(Vec::new());
    }
    reject_symlink(&graders_dir)?;
    let mut entries: Vec<PathBuf> = fs::read_dir(&graders_dir)
        .map_err(|error| format!("failed to read {}: {error}", graders_dir.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("md"))
        .collect();
    entries.sort();
    entries
        .into_iter()
        .map(|path| {
            reject_symlink(&path)?;
            let raw = read_bounded(&path, MAX_PROMPT_BYTES)?;
            let (frontmatter, body) = parse_markdown_frontmatter(&raw, &path)?;
            grader_from_parts(
                &frontmatter,
                body,
                path.file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
            )
        })
        .collect()
}

fn parse_yaml_graders(value: &YamlValue, case_dir: &Path) -> Result<Vec<GraderDefinition>, String> {
    let values = value
        .as_sequence()
        .ok_or_else(|| "graders must be an array".to_string())?;
    values
        .iter()
        .enumerate()
        .map(|(index, grader)| parse_yaml_grader(grader, case_dir, index))
        .collect()
}

fn parse_yaml_grader(
    value: &YamlValue,
    case_dir: &Path,
    index: usize,
) -> Result<GraderDefinition, String> {
    if let Some(path) = value.as_str() {
        let path = confined_relative(case_dir, Path::new(path))?;
        let raw = read_bounded(&path, MAX_PROMPT_BYTES)?;
        let (frontmatter, body) = parse_markdown_frontmatter(&raw, &path)?;
        return grader_from_parts(
            &frontmatter,
            body,
            path.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
        );
    }
    let map = value
        .as_mapping()
        .ok_or_else(|| format!("grader[{index}] must be a string or object"))?;
    let kind = yaml_string(map, "type").unwrap_or_else(|| "llm".to_string());
    let name = yaml_string(map, "name").unwrap_or_else(|| format!("grader-{}", index + 1));
    let weight = yaml_f64(map, "weight").unwrap_or(1.0);
    if !weight.is_finite() || weight <= 0.0 {
        return Err(format!("grader {name:?} weight must be positive"));
    }
    match kind.as_str() {
        "regex" => {
            let pattern = grader_value(map)?;
            regex::Regex::new(&pattern)
                .map_err(|error| format!("invalid regex grader {name:?}: {error}"))?;
            Ok(GraderDefinition::Regex {
                name,
                pattern,
                weight,
            })
        }
        "tool_order" => {
            let tools = yaml_strings(map, "tools")?;
            if tools.is_empty() {
                return Err(format!("tool_order grader {name:?} requires tools"));
            }
            Ok(GraderDefinition::ToolOrder {
                name,
                tools,
                weight,
            })
        }
        "tool_used" => Ok(GraderDefinition::ToolUsed {
            name,
            tool: yaml_string(map, "tool")
                .or_else(|| yaml_string(map, "value"))
                .ok_or_else(|| "tool_used grader requires tool".to_string())?,
            weight,
        }),
        "file_exists" => Ok(GraderDefinition::FileExists {
            name,
            path: PathBuf::from(
                yaml_string(map, "path")
                    .or_else(|| yaml_string(map, "value"))
                    .ok_or_else(|| "file_exists grader requires path".to_string())?,
            ),
            weight,
        }),
        "llm" | "baseline" => {
            let rubric = if let Some(rubric) =
                yaml_string(map, "rubric").or_else(|| yaml_string(map, "prompt"))
            {
                rubric
            } else if let Some(path) = yaml_string(map, "path") {
                read_bounded(
                    &confined_relative(case_dir, Path::new(&path))?,
                    MAX_PROMPT_BYTES,
                )?
            } else {
                return Err(format!(
                    "LLM grader {name:?} requires rubric, prompt, or path"
                ));
            };
            if kind == "baseline" {
                Ok(GraderDefinition::Baseline {
                    name,
                    rubric,
                    weight,
                })
            } else {
                Ok(GraderDefinition::Llm {
                    name,
                    rubric,
                    weight,
                })
            }
        }
        other => Err(format!("unsupported grader type {other:?}")),
    }
}

fn grader_from_parts(
    frontmatter: &Mapping,
    body: String,
    default_name: String,
) -> Result<GraderDefinition, String> {
    let kind = yaml_string(frontmatter, "type").unwrap_or_else(|| "llm".to_string());
    let name = yaml_string(frontmatter, "name").unwrap_or(default_name);
    let weight = yaml_f64(frontmatter, "weight").unwrap_or(1.0);
    if !weight.is_finite() || weight <= 0.0 {
        return Err(format!("grader {name:?} weight must be positive"));
    }
    match kind.as_str() {
        "regex" => {
            let pattern = yaml_string(frontmatter, "pattern").unwrap_or(body);
            regex::Regex::new(&pattern)
                .map_err(|error| format!("invalid regex grader {name:?}: {error}"))?;
            Ok(GraderDefinition::Regex {
                name,
                pattern,
                weight,
            })
        }
        "tool_order" => {
            let tools = yaml_strings(frontmatter, "tools")?;
            if tools.is_empty() {
                return Err(format!("tool_order grader {name:?} requires tools"));
            }
            Ok(GraderDefinition::ToolOrder {
                name,
                tools,
                weight,
            })
        }
        "tool_used" => Ok(GraderDefinition::ToolUsed {
            name,
            tool: yaml_string(frontmatter, "tool").unwrap_or_else(|| body.trim().to_string()),
            weight,
        }),
        "file_exists" => Ok(GraderDefinition::FileExists {
            name,
            path: PathBuf::from(
                yaml_string(frontmatter, "path").unwrap_or_else(|| body.trim().to_string()),
            ),
            weight,
        }),
        "llm" => Ok(GraderDefinition::Llm {
            name,
            rubric: body,
            weight,
        }),
        "baseline" => Ok(GraderDefinition::Baseline {
            name,
            rubric: body,
            weight,
        }),
        other => Err(format!("unsupported grader type {other:?}")),
    }
}

fn parse_markdown_frontmatter(raw: &str, path: &Path) -> Result<(Mapping, String), String> {
    let normalized = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    if !normalized.starts_with("---\n") {
        return Ok((Mapping::new(), normalized.to_string()));
    }
    let rest = &normalized[4..];
    let Some(end) = rest.find("\n---\n") else {
        return Err(format!(
            "{} has unterminated YAML frontmatter",
            path.display()
        ));
    };
    let frontmatter: YamlValue = serde_yaml::from_str(&rest[..end])
        .map_err(|error| format!("invalid frontmatter in {}: {error}", path.display()))?;
    let mapping = frontmatter
        .as_mapping()
        .cloned()
        .ok_or_else(|| format!("frontmatter in {} must be an object", path.display()))?;
    Ok((
        mapping,
        rest[end + 5..].trim_start_matches('\n').to_string(),
    ))
}

fn grader_value(map: &Mapping) -> Result<String, String> {
    yaml_string(map, "value")
        .or_else(|| yaml_string(map, "expected"))
        .or_else(|| yaml_string(map, "pattern"))
        .ok_or_else(|| "deterministic grader requires value/expected/pattern".to_string())
}

fn yaml_value<'a>(map: &'a Mapping, key: &str) -> Option<&'a YamlValue> {
    map.get(YamlValue::String(key.to_string()))
}

fn yaml_string(map: &Mapping, key: &str) -> Option<String> {
    yaml_value(map, key)
        .and_then(YamlValue::as_str)
        .map(str::to_string)
}

fn yaml_u64(map: &Mapping, key: &str) -> Option<u64> {
    yaml_value(map, key).and_then(YamlValue::as_u64)
}

fn yaml_f64(map: &Mapping, key: &str) -> Option<f64> {
    yaml_value(map, key).and_then(|value| value.as_f64())
}

fn yaml_strings(map: &Mapping, key: &str) -> Result<Vec<String>, String> {
    let Some(value) = yaml_value(map, key) else {
        return Ok(Vec::new());
    };
    if let Some(single) = value.as_str() {
        return Ok(vec![single.to_string()]);
    }
    value
        .as_sequence()
        .ok_or_else(|| format!("{key} must be a string or array"))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("{key} entries must be strings"))
        })
        .collect()
}

fn selected_case(case: &EvalCase, cli: &Cli) -> bool {
    let name_matches = cli
        .case_filter
        .as_deref()
        .is_none_or(|pattern| glob_matches(pattern, &case.name));
    let tags_match = cli
        .tag
        .iter()
        .all(|wanted| case.tags.iter().any(|tag| tag == wanted));
    name_matches && tags_match
}

fn glob_matches(pattern: &str, value: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let value: Vec<char> = value.chars().collect();
    let mut table = vec![vec![false; value.len() + 1]; pattern.len() + 1];
    table[0][0] = true;
    for i in 1..=pattern.len() {
        if pattern[i - 1] == '*' {
            table[i][0] = table[i - 1][0];
        }
        for j in 1..=value.len() {
            table[i][j] = match pattern[i - 1] {
                '*' => table[i - 1][j] || table[i][j - 1],
                '?' => table[i - 1][j - 1],
                literal => table[i - 1][j - 1] && literal == value[j - 1],
            };
        }
    }
    table[pattern.len()][value.len()]
}

async fn run_init(args: &InitArgs, parent_eval_dir: Option<&str>, cwd: &Path) -> i32 {
    let name = args.name.as_deref().unwrap_or("eval");
    // `evalDir: c.evalDir ?? a.opts().evalDir` — init's own flag, else the
    // parent `plugin eval --eval-dir`, else the manifest / `evals/`.
    let eval_dir = match resolve_eval_dir(args.eval_dir.as_deref().or(parent_eval_dir), cwd) {
        Ok(dir) => dir,
        Err(error) => {
            eprintln!("lingxi-cli plugin eval init: {error}");
            return RUNTIME_ERROR;
        }
    };
    let suite_dir = match eval_suite_dir(cwd, &eval_dir, name) {
        Ok(path) => path,
        Err(error) => {
            eprintln!("lingxi-cli plugin eval init: {error}");
            return RUNTIME_ERROR;
        }
    };
    if args.bare {
        return match write_bare_template(&suite_dir, name) {
            Ok(()) => {
                println!(
                    "Created {eval_dir}/{name}/prompt.md and {eval_dir}/{name}/graders/criteria.md"
                );
                SUCCESS
            }
            Err(error) => {
                eprintln!("lingxi-cli plugin eval init: {error}");
                RUNTIME_ERROR
            }
        };
    }

    // `o = !t.bare && (t.forceInteractive || isTTY)`, then `if (o && !isTTY)`
    // refuse. `--bare` already returned above, so the reachable half of the
    // oracle's gate is: an EXPLICIT `-i/--interactive` (or its hidden
    // `--interview` alias) off-TTY is refused with the byte-exact copy (product
    // noun rebranded `claude` → `lingxi-cli`).
    if (args.interactive || args.interview)
        && !(std::io::stdin().is_terminal() && std::io::stdout().is_terminal())
    {
        if let Some(case_name) = args.name.as_deref() {
            eprintln!(
                "The authoring interview requires an interactive terminal (TTY). Run `lingxi-cli plugin eval init` in a terminal, or drop --interactive to write a blank template for `{case_name}` instead."
            );
        } else {
            eprintln!(
                "The authoring interview requires an interactive terminal (TTY). Run `lingxi-cli plugin eval init` in a terminal, or drop --interactive and pass a case name (e.g. `lingxi-cli plugin eval init my-case`) to write a blank template instead."
            );
        }
        return RUNTIME_ERROR;
    }

    if suite_dir.exists() {
        eprintln!(
            "lingxi-cli plugin eval init: {} already exists",
            suite_dir.display()
        );
        return RUNTIME_ERROR;
    }
    let staging = match RunTemp::create(false) {
        Ok(staging) => staging,
        Err(error) => {
            eprintln!("lingxi-cli plugin eval init: {error}");
            return RUNTIME_ERROR;
        }
    };
    let staged_plugin = staging.path.join("plugin");
    if let Err(error) = copy_plugin_snapshot(cwd, &staged_plugin) {
        eprintln!("lingxi-cli plugin eval init: {error}");
        return RUNTIME_ERROR;
    }
    let prompt = format!(
        "Author an evaluation suite named {name:?} for the plugin snapshot in the current \
         directory. Interview the user \
         to source representative inputs and design objective graders. Write only beneath \
         {eval_dir}/{name}/, using prompt.md plus graders/*.md or case.yaml. Do not modify plugin \
         runtime code. The snapshot is isolated; only the validated eval suite will be copied \
         back to the original plugin."
    );
    let executable = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("lingxi-cli plugin eval init: failed to resolve current executable: {error}");
            return RUNTIME_ERROR;
        }
    };
    let status = match tokio::process::Command::new(executable)
        .arg("--cwd")
        .arg(&staged_plugin)
        .arg(prompt)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await
    {
        Ok(status) if status.success() => status,
        Ok(status) => {
            eprintln!("lingxi-cli plugin eval init: authoring session exited with {status}");
            return RUNTIME_ERROR;
        }
        Err(error) => {
            eprintln!("lingxi-cli plugin eval init: failed to start authoring session: {error}");
            return RUNTIME_ERROR;
        }
    };
    debug_assert!(status.success());
    let staged_suite = staged_plugin.join(&eval_dir).join(name);
    if let Err(error) = validate_authored_suite(&staged_plugin, &eval_dir, &staged_suite) {
        eprintln!("lingxi-cli plugin eval init: {error}");
        return RUNTIME_ERROR;
    }
    match install_authored_suite(&staged_suite, &suite_dir) {
        Ok(()) => {
            println!("Created {eval_dir}/{name}/");
            SUCCESS
        }
        Err(error) => {
            eprintln!("lingxi-cli plugin eval init: {error}");
            RUNTIME_ERROR
        }
    }
}

fn copy_plugin_snapshot(source: &Path, destination: &Path) -> Result<(), String> {
    let source = fs::canonicalize(source)
        .map_err(|error| format!("failed to resolve {}: {error}", source.display()))?;
    fs::create_dir(destination)
        .map_err(|error| format!("failed to create staging directory: {error}"))?;
    set_private_dir_permissions(destination)?;
    let mut pending = vec![(source, destination.to_path_buf(), true)];
    let mut file_count = 0usize;
    let mut total_bytes = 0u64;
    while let Some((src, dst, is_root)) = pending.pop() {
        for entry in fs::read_dir(&src)
            .map_err(|error| format!("failed to read plugin snapshot {}: {error}", src.display()))?
        {
            let entry =
                entry.map_err(|error| format!("failed to read plugin snapshot: {error}"))?;
            let name = entry.file_name();
            if is_root
                && matches!(
                    name.to_str(),
                    Some(".git" | ".omx" | "target" | "node_modules")
                )
            {
                continue;
            }
            let from = entry.path();
            let to = dst.join(&name);
            let ty = entry
                .file_type()
                .map_err(|error| format!("failed to inspect {}: {error}", from.display()))?;
            if ty.is_symlink() {
                return Err(format!(
                    "refusing symlink or reparse point in plugin snapshot: {}",
                    from.display()
                ));
            }
            file_count += 1;
            if file_count > MAX_STAGE_FILES {
                return Err(format!(
                    "plugin snapshot exceeded the {MAX_STAGE_FILES}-entry safety limit"
                ));
            }
            if ty.is_dir() {
                fs::create_dir(&to)
                    .map_err(|error| format!("failed to create {}: {error}", to.display()))?;
                pending.push((from, to, false));
            } else if ty.is_file() {
                let size = entry
                    .metadata()
                    .map_err(|error| format!("failed to inspect {}: {error}", from.display()))?
                    .len();
                total_bytes = total_bytes.saturating_add(size);
                if total_bytes > MAX_STAGE_BYTES {
                    return Err(format!(
                        "plugin snapshot exceeded the {} MiB safety limit",
                        MAX_STAGE_BYTES / (1024 * 1024)
                    ));
                }
                fs::copy(&from, &to)
                    .map_err(|error| format!("failed to copy {}: {error}", from.display()))?;
            }
        }
    }
    Ok(())
}

fn validate_authored_suite(plugin_root: &Path, eval_dir: &str, suite: &Path) -> Result<(), String> {
    if !suite.is_dir() {
        return Err(format!(
            "authoring session did not create {}",
            suite.display()
        ));
    }
    validate_tree_confined(suite)?;
    let cases = discover_cases(plugin_root, eval_dir)?;
    if !cases.iter().any(|case| case.source.starts_with(suite)) {
        return Err("authoring session did not create a valid eval case".to_string());
    }
    Ok(())
}

fn install_authored_suite(source: &Path, destination: &Path) -> Result<(), String> {
    if destination.exists() {
        return Err(format!("{} already exists", destination.display()));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| format!("{} has no parent", destination.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    reject_symlink(parent)?;
    let staged = parent.join(format!(
        ".{}.{}.tmp",
        destination
            .file_name()
            .unwrap_or_default()
            .to_string_lossy(),
        uuid::Uuid::new_v4()
    ));
    copy_plugin_snapshot(source, &staged)?;
    fs::rename(&staged, destination)
        .map_err(|error| format!("failed to install {}: {error}", destination.display()))
}

fn eval_suite_dir(cwd: &Path, eval_dir: &str, name: &str) -> Result<PathBuf, String> {
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || name == "."
    {
        return Err(format!("invalid eval suite name {name:?}"));
    }
    Ok(cwd.join(eval_dir).join(name))
}

/// Resolve the eval directory that holds the cases: `--eval-dir` wins,
/// otherwise the plugin manifest's `experimental.evals` value, otherwise
/// `evals/` — 1:1 with the oracle's "(default dir: the manifest's
/// experimental.evals value, else evals/)".
fn resolve_eval_dir(flag: Option<&str>, plugin_root: &Path) -> Result<String, String> {
    if let Some(name) = flag {
        return validate_eval_dir_name(name);
    }
    match manifest_eval_dir(plugin_root) {
        Some(name) => validate_eval_dir_name(&name),
        None => Ok(DEFAULT_EVAL_DIR.to_string()),
    }
}

/// `experimental.evals` from the plugin manifest at `<root>/<manifest dir>/
/// plugin.json`, when it is a non-empty string.
fn manifest_eval_dir(plugin_root: &Path) -> Option<String> {
    let manifest = plugin_root
        .join(branding::PLUGIN_MANIFEST_DIR)
        .join("plugin.json");
    let text = fs::read_to_string(manifest).ok()?;
    let value = serde_json::from_str::<serde_json::Value>(&text).ok()?;
    value
        .get("experimental")?
        .get("evals")?
        .as_str()
        .map(str::to_string)
}

/// The eval dir names ONE directory below the plugin root — no separators, no
/// traversal. A trailing `/` (the manifest's `evals/` spelling) is tolerated.
fn validate_eval_dir_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim().trim_end_matches('/');
    if trimmed.is_empty()
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains("..")
        || trimmed == "."
    {
        return Err(format!("invalid eval dir {name:?}"));
    }
    Ok(trimmed.to_string())
}

fn write_bare_template(suite_dir: &Path, _name: &str) -> Result<(), String> {
    if suite_dir.exists() {
        return Err(format!("{} already exists", suite_dir.display()));
    }
    fs::create_dir_all(suite_dir.join("graders"))
        .map_err(|error| format!("failed to create {}: {error}", suite_dir.display()))?;
    set_private_dir_permissions(suite_dir)?;
    atomic_write(
        &suite_dir.join("prompt.md"),
        b"---\nmax_turns: 10\nallowed_tools: [Read, Glob, Grep, Skill]\n---\n\nTODO: describe what the agent should do\n",
    )?;
    atomic_write(
        &suite_dir.join("graders").join("criteria.md"),
        b"---\ntype: llm\nweight: 1\n---\n\nTODO: describe what a successful response looks like\n",
    )
}

fn emit_outputs(cli: &Cli, result: &AggregateResult, cwd: &Path) -> Result<(), String> {
    let json = serde_json::to_string_pretty(result)
        .map_err(|error| format!("failed to serialize eval result: {error}"))?;
    // "results go to … ./<dir>/results/ with this flag, else ./evals/results/"
    // — the CWD-relative results dir tracks the FLAG, not a manifest default.
    let results_root = cli
        .eval_dir
        .as_deref()
        .map(validate_eval_dir_name)
        .transpose()?
        .unwrap_or_else(|| DEFAULT_EVAL_DIR.to_string());
    let output_dir = cli.output_dir.clone().unwrap_or_else(|| {
        cwd.join(&results_root)
            .join("results")
            .join(chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string())
    });
    fs::create_dir_all(&output_dir)
        .map_err(|error| format!("failed to create {}: {error}", output_dir.display()))?;
    atomic_write(&output_dir.join("aggregate-result.json"), json.as_bytes())?;

    if let Some(path) = &cli.report {
        atomic_write(
            &resolve_output_path(cwd, path),
            render_html_report(result).as_bytes(),
        )?;
    }
    match cli.json.as_deref() {
        Some("-") => println!("{json}"),
        Some(path) => atomic_write(&resolve_output_path(cwd, Path::new(path)), json.as_bytes())?,
        None => {
            if result.cases.is_empty() {
                return Ok(());
            }
            println!(
                "Plugin eval: {:.3} score · ${:.4} · {} case(s){}",
                result.aggregates.overall_score,
                result.cost_usd,
                result.cases.len(),
                if result.partial { " · partial" } else { "" }
            );
            for case in &result.cases {
                if let Some(delta) = case.aggregates.delta {
                    println!(
                        "  {}: {:.3} (baseline {:.3}, delta {delta:+.3})",
                        case.name,
                        case.aggregates.score,
                        case.aggregates.score_without.unwrap_or_default()
                    );
                } else {
                    println!("  {}: {:.3}", case.name, case.aggregates.score);
                }
            }
            println!("Results: {}", output_dir.display());
        }
    }
    Ok(())
}

fn resolve_output_path(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

fn render_html_report(result: &AggregateResult) -> String {
    let mut rows = String::new();
    let mut details = String::new();
    for case in &result.cases {
        let baseline = case
            .aggregates
            .score_without
            .map_or_else(|| "—".to_string(), |score| format!("{score:.3}"));
        let delta = case
            .aggregates
            .delta
            .map_or_else(|| "—".to_string(), |score| format!("{score:+.3}"));
        rows.push_str(&format!(
            "<tr><td>{}</td><td>{:.3}</td><td>{baseline}</td><td>{delta}</td></tr>",
            html_escape(&case.name),
            case.aggregates.score
        ));
        details.push_str(&format!(
            "<section><h2>{}</h2><h3>Prompt</h3><pre>{}</pre>",
            html_escape(&case.name),
            html_escape(&case.prompt_markdown)
        ));
        for run in case
            .arms
            .with_plugin
            .iter()
            .chain(case.arms.without_plugin.iter().flatten())
        {
            details.push_str(&format!(
                "<details><summary>{} run {} · score {:.3}</summary><h4>Output</h4><pre>{}</pre>",
                html_escape(&run.arm),
                run.run,
                run.score,
                html_escape(&run.output)
            ));
            if let Some(error) = &run.error {
                details.push_str(&format!("<h4>Error</h4><pre>{}</pre>", html_escape(error)));
            }
            details.push_str("<h4>Grader verdicts</h4><ul>");
            for grader in &run.graders {
                details.push_str(&format!(
                    "<li><strong>{}</strong>: {} — {}</li>",
                    html_escape(&grader.name),
                    grader
                        .score
                        .map_or_else(|| "skipped".to_string(), |score| format!("{score:.3}")),
                    html_escape(&grader.reason)
                ));
            }
            details.push_str("</ul></details>");
        }
        details.push_str("</section>");
    }
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Plugin eval</title>\
         <style>body{{font:14px system-ui;margin:2rem;max-width:960px}}table{{border-collapse:collapse;\
         width:100%}}th,td{{border:1px solid #ccc;padding:.5rem;text-align:left}}</style></head>\
         <body><h1>Plugin eval</h1><p>Target: <code>{}</code></p><p>Score: {:.3} · Cost: \
         ${:.4}</p><table><thead><tr><th>Case</th><th>Plugin</th><th>Baseline</th>\
         <th>Delta</th></tr></thead><tbody>{rows}</tbody></table>{details}</body></html>",
        html_escape(
            result
                .suite
                .plugin_id
                .as_deref()
                .unwrap_or_else(|| result.suite.root.to_str().unwrap_or(""))
        ),
        result.aggregates.overall_score,
        result.cost_usd
    )
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    reject_symlink(parent)?;
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(format!(
                "refusing to replace non-regular output {}",
                path.display()
            ));
        }
    }
    let temp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy(),
        uuid::Uuid::new_v4()
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = options
        .open(&temp)
        .map_err(|error| format!("failed to create {}: {error}", temp.display()))?;
    set_private_file_permissions(&temp)?;
    let result = (|| {
        file.write_all(bytes)
            .map_err(|error| format!("failed to write {}: {error}", temp.display()))?;
        file.sync_all()
            .map_err(|error| format!("failed to sync {}: {error}", temp.display()))?;
        fs::rename(&temp, path)
            .map_err(|error| format!("failed to replace {}: {error}", path.display()))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn confined_relative(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(format!(
            "grader path {} escapes its case",
            relative.display()
        ));
    }
    let path = root.join(relative);
    reject_symlink(&path)?;
    let canonical = fs::canonicalize(&path)
        .map_err(|error| format!("failed to resolve {}: {error}", path.display()))?;
    let canonical_root = fs::canonicalize(root)
        .map_err(|error| format!("failed to resolve {}: {error}", root.display()))?;
    if !canonical.starts_with(canonical_root) {
        return Err(format!("grader path {} escapes its case", path.display()));
    }
    Ok(canonical)
}

fn reject_symlink(path: &Path) -> Result<(), String> {
    if fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(format!("symlink path is not allowed: {}", path.display()));
    }
    Ok(())
}

fn read_bounded(path: &Path, max: usize) -> Result<String, String> {
    reject_symlink(path)?;
    let metadata = fs::metadata(path)
        .map_err(|error| format!("failed to inspect {}: {error}", path.display()))?;
    if metadata.len() > max as u64 {
        return Err(format!("{} exceeds the {max}-byte limit", path.display()));
    }
    fs::read_to_string(path).map_err(|error| format!("failed to read {}: {error}", path.display()))
}

fn safe_segment(value: &str) -> String {
    let mut out: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect();
    if out.is_empty() {
        out.push_str("case");
    }
    out.truncate(80);
    out
}

fn truncate_utf8(value: &str, max: usize) -> &str {
    if value.len() <= max {
        return value;
    }
    let mut end = max;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn parse_positive_f64(value: &str) -> Result<f64, String> {
    let parsed: f64 = value
        .parse()
        .map_err(|_| "value must be a positive finite number".to_string())?;
    if parsed.is_finite() && parsed > 0.0 {
        Ok(parsed)
    } else {
        Err("value must be a positive finite number".to_string())
    }
}

fn parse_positive_u32(value: &str) -> Result<u32, String> {
    let parsed: u32 = value
        .parse()
        .map_err(|_| "value must be a positive integer".to_string())?;
    if parsed == 0 {
        Err("value must be a positive integer".to_string())
    } else {
        Ok(parsed)
    }
}

fn parse_threshold(value: &str) -> Result<f64, String> {
    let parsed: f64 = value
        .parse()
        .map_err(|_| "--threshold must be a number from 0 to 1".to_string())?;
    if parsed.is_finite() && (0.0..=1.0).contains(&parsed) {
        Ok(parsed)
    } else {
        Err("--threshold must be a number from 0 to 1".to_string())
    }
}

#[cfg(unix)]
fn set_private_dir_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("failed to secure {}: {error}", path.display()))
}

#[cfg(not(unix))]
fn set_private_dir_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("failed to secure {}: {error}", path.display()))
}

#[cfg(not(unix))]
fn set_private_file_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::argv::Argv;
    use clap::Parser as _;

    #[test]
    fn clap_surface_accepts_eval_options_and_init_bare() {
        let parsed = Argv::from_iter([
            "lingxi-cli",
            "plugin",
            "eval",
            "--json",
            "result.json",
            "--model",
            "opus",
            "--ablation",
            "with-without",
            "--runs",
            "2",
            ".",
        ])
        .unwrap();
        let crate::commands::Commands::Plugin(plugin) = parsed.command.unwrap() else {
            panic!("expected plugin command");
        };
        let crate::commands::plugin::Sub::Eval(eval) = plugin.command.unwrap() else {
            panic!("expected eval command");
        };
        assert_eq!(eval.json.as_deref(), Some("result.json"));
        assert_eq!(eval.model.as_deref(), Some("opus"));
        assert_eq!(eval.runs, Some(2));
        assert_eq!(eval.target.as_deref(), Some("."));

        let parsed =
            Argv::from_iter(["lingxi-cli", "plugin", "eval", "init", "--bare", "smoke"]).unwrap();
        let crate::commands::Commands::Plugin(plugin) = parsed.command.unwrap() else {
            panic!("expected plugin command");
        };
        let crate::commands::plugin::Sub::Eval(eval) = plugin.command.unwrap() else {
            panic!("expected eval command");
        };
        assert!(matches!(
            eval.command,
            Some(EvalSub::Init(InitArgs {
                bare: true,
                name: Some(ref name),
                ..
            })) if name == "smoke"
        ));
    }

    // CLI-04/CLI-05 (cc 2.1.238): the four flags 2.1.238 added to the
    // `plugin eval` family, each with 0 hits in 2.1.220 —
    // `eval --eval-dir <dir>`, `eval --no-publish`, `eval init -i/--interactive`
    // (plus its hidden `--interview` alias), and `eval init --eval-dir <dir>`.
    #[test]
    fn clap_surface_accepts_the_2_1_238_eval_flags() {
        let eval = parse_eval([
            "lingxi-cli",
            "plugin",
            "eval",
            "--eval-dir",
            "checks",
            "--no-publish",
            ".",
        ]);
        assert_eq!(eval.eval_dir.as_deref(), Some("checks"));
        assert!(eval.no_publish);

        // init's own `--eval-dir` plus `-i`.
        let eval = parse_eval([
            "lingxi-cli",
            "plugin",
            "eval",
            "init",
            "-i",
            "--eval-dir",
            "checks",
            "smoke",
        ]);
        let Some(EvalSub::Init(init)) = eval.command else {
            panic!("expected eval init");
        };
        assert!(init.interactive);
        assert!(!init.interview);
        assert_eq!(init.eval_dir.as_deref(), Some("checks"));

        // The hidden `--interview` alias parses and is OR-ed with
        // `--interactive` by `run_init`.
        let eval = parse_eval(["lingxi-cli", "plugin", "eval", "init", "--interview"]);
        let Some(EvalSub::Init(init)) = eval.command else {
            panic!("expected eval init");
        };
        assert!(init.interview);
        assert!(!init.interactive);
    }

    fn parse_eval<'a>(argv: impl IntoIterator<Item = &'a str>) -> Cli {
        let parsed = Argv::from_iter(argv).unwrap();
        let crate::commands::Commands::Plugin(plugin) = parsed.command.unwrap() else {
            panic!("expected plugin command");
        };
        let crate::commands::plugin::Sub::Eval(eval) = plugin.command.unwrap() else {
            panic!("expected eval command");
        };
        eval
    }

    // CLI-04: the eval dir resolves flag → manifest `experimental.evals` →
    // `evals/`, and `discover_cases` really looks below that name.
    #[test]
    fn eval_dir_resolves_flag_then_manifest_then_default() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        assert_eq!(resolve_eval_dir(None, root).unwrap(), DEFAULT_EVAL_DIR);
        assert_eq!(resolve_eval_dir(Some("checks"), root).unwrap(), "checks");

        let manifest_dir = root.join(branding::PLUGIN_MANIFEST_DIR);
        fs::create_dir_all(&manifest_dir).unwrap();
        fs::write(
            manifest_dir.join("plugin.json"),
            br#"{"name":"p","experimental":{"evals":"nested/suites"}}"#,
        )
        .unwrap();
        // A manifest value naming a nested path is refused, not silently joined.
        assert!(resolve_eval_dir(None, root).is_err());

        // The manifest's `evals/` spelling (trailing slash) is tolerated.
        fs::write(
            manifest_dir.join("plugin.json"),
            br#"{"name":"p","experimental":{"evals":"suites/"}}"#,
        )
        .unwrap();
        assert_eq!(resolve_eval_dir(None, root).unwrap(), "suites");
        // The flag still wins over the manifest.
        assert_eq!(resolve_eval_dir(Some("checks"), root).unwrap(), "checks");

        // Traversal is refused wherever it comes from.
        assert!(resolve_eval_dir(Some("../escape"), root).is_err());
        assert!(resolve_eval_dir(Some(""), root).is_err());

        // And the resolved name is what discovery walks.
        let suite = root.join("suites").join("case");
        fs::create_dir_all(suite.join("graders")).unwrap();
        fs::write(suite.join("prompt.md"), "do the thing").unwrap();
        fs::write(
            suite.join("graders").join("criteria.md"),
            "---\ntype: llm\nweight: 1\n---\n\nlooks right\n",
        )
        .unwrap();
        assert!(discover_cases(root, DEFAULT_EVAL_DIR).unwrap().is_empty());
        assert_eq!(discover_cases(root, "suites").unwrap().len(), 1);
    }

    #[test]
    fn bare_json_flag_uses_stdout_sentinel() {
        let parsed = Argv::from_iter(["lingxi-cli", "plugin", "eval", "--json"]).unwrap();
        let crate::commands::Commands::Plugin(plugin) = parsed.command.unwrap() else {
            panic!("expected plugin command");
        };
        let crate::commands::plugin::Sub::Eval(eval) = plugin.command.unwrap() else {
            panic!("expected eval command");
        };
        assert_eq!(eval.json.as_deref(), Some("-"));
        assert_eq!(eval.target, None);
    }

    #[test]
    fn discovers_yaml_and_markdown_cases_and_rejects_duplicates() {
        let temp = tempfile::tempdir().unwrap();
        let evals = temp.path().join("evals");
        fs::create_dir_all(evals.join("yaml")).unwrap();
        fs::write(
            evals.join("yaml").join("case.yaml"),
            "name: yaml-case\nprompt: say hello\ntags: [smoke]\nruns: 2\ngraders:\n  - type: regex\n    pattern: hello\n",
        )
        .unwrap();
        fs::create_dir_all(evals.join("markdown").join("graders")).unwrap();
        fs::write(evals.join("markdown").join("prompt.md"), "review this code").unwrap();
        fs::write(
            evals.join("markdown").join("graders").join("criteria.md"),
            "must find the bug",
        )
        .unwrap();

        let cases = discover_cases(temp.path(), DEFAULT_EVAL_DIR).unwrap();
        assert_eq!(cases.len(), 2);
        assert_eq!(cases[0].name, "markdown");
        assert!(matches!(cases[0].graders[0], GraderDefinition::Llm { .. }));
        assert_eq!(cases[1].name, "yaml-case");
        assert_eq!(cases[1].runs, Some(2));
        assert!(matches!(
            cases[1].graders[0],
            GraderDefinition::Regex { .. }
        ));
    }

    #[test]
    fn named_install_resolves_v2_install_path() {
        let temp = tempfile::tempdir().unwrap();
        let plugins = temp.path().join("plugins");
        let installed = temp.path().join("cache").join("demo");
        fs::create_dir_all(&installed).unwrap();
        fs::create_dir_all(&plugins).unwrap();
        fs::write(
            plugins.join("installed_plugins.json"),
            serde_json::json!({
                "version": 2,
                "plugins": {
                    "demo@official": [{
                        "scope": "user",
                        "installPath": installed
                    }]
                }
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            resolve_installed_target("demo@official", &plugins).unwrap(),
            Some(installed.clone())
        );
        assert_eq!(
            resolve_installed_target("demo", &plugins).unwrap(),
            Some(installed)
        );
    }

    #[test]
    fn scaffold_is_opt_in_and_threshold_is_bounded() {
        assert!(parse_threshold("0").is_ok());
        assert!(parse_threshold("1").is_ok());
        assert!(parse_threshold("1.01").is_err());
        assert!(Argv::try_parse_from([
            "lingxi-cli",
            "plugin",
            "eval",
            "--scaffold",
            "--no-scaffold",
            "."
        ])
        .is_err());
    }

    #[test]
    fn eval_agent_output_requires_one_terminal_result_frame() {
        let missing =
            parse_agent_stream_output("{\"type\":\"assistant\",\"message\":{\"content\":[]}}\n");
        assert!(missing.unwrap_err().contains("terminal result"));

        let malformed = parse_agent_stream_output("{not json}\n");
        assert!(malformed.unwrap_err().contains("invalid stream-json"));

        let duplicate = parse_agent_stream_output(
            "{\"type\":\"result\",\"result\":\"a\",\"is_error\":false,\"duration_ms\":1,\"num_turns\":1,\"total_cost_usd\":0.1}\n\
             {\"type\":\"result\",\"result\":\"b\",\"is_error\":false,\"duration_ms\":2,\"num_turns\":2,\"total_cost_usd\":0.2}\n",
        );
        assert!(duplicate.unwrap_err().contains("exactly one"));
    }

    #[test]
    fn eval_agent_output_accepts_complete_terminal_result() {
        let parsed = parse_agent_stream_output(
            "{\"type\":\"assistant\",\"message\":{\"content\":[]}}\n\
             {\"type\":\"result\",\"result\":\"done\",\"is_error\":false,\"duration_ms\":250,\"num_turns\":2,\"total_cost_usd\":0.25}\n",
        )
        .expect("valid stream");
        assert_eq!(parsed.text, "done");
        assert_eq!(parsed.turns, 2);
        assert_eq!(parsed.duration_seconds, 0.25);
        assert_eq!(parsed.cost_usd, 0.25);
        assert!(!parsed.is_error);
    }

    #[test]
    fn scaffold_policy_is_fail_closed_and_confined_to_run_root() {
        let root = Path::new("/tmp/lingxi-eval-run");
        let denied = vec![PathBuf::from("/home/user/.ssh")];
        let policy = scaffold_sandbox_policy(root, &denied);
        assert_eq!(policy["enabled"], true);
        assert_eq!(policy["failIfUnavailable"], true);
        assert_eq!(policy["allowUnsandboxedCommands"], false);
        assert_eq!(policy["network"]["allowedDomains"], serde_json::json!([]));
        assert_eq!(
            policy["filesystem"]["allowWrite"],
            serde_json::json!(["/tmp/lingxi-eval-run"])
        );
        assert_eq!(
            policy["filesystem"]["allowRead"],
            serde_json::json!(["/tmp/lingxi-eval-run"])
        );
        assert_eq!(
            policy["filesystem"]["denyRead"],
            serde_json::json!(["/home/user/.ssh"])
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[tokio::test]
    async fn scaffold_runtime_writes_inside_root_and_denies_parent_escape() {
        let temp = tempfile::tempdir().unwrap();
        let run_root = temp.path().join("run");
        fs::create_dir(&run_root).unwrap();

        if let Err(error) = run_scaffold("printf ok > inside.txt", &run_root).await {
            assert!(
                error.contains("sandbox") || error.contains("unavailable"),
                "sandbox must either enforce or fail closed: {error}"
            );
            return;
        }
        assert_eq!(
            fs::read_to_string(run_root.join("inside.txt")).unwrap(),
            "ok"
        );

        let escaped = temp.path().join("escaped.txt");
        let error = run_scaffold("printf escaped > ../escaped.txt", &run_root)
            .await
            .expect_err("sandbox must deny writes outside the eval run root");
        assert!(error.contains("scaffold script failed"));
        assert!(!escaped.exists());
    }

    #[test]
    fn init_template_is_atomic_and_refuses_existing_suite() {
        let temp = tempfile::tempdir().unwrap();
        let suite = eval_suite_dir(temp.path(), DEFAULT_EVAL_DIR, "smoke").unwrap();
        write_bare_template(&suite, "smoke").unwrap();
        assert_eq!(
            fs::read(suite.join("prompt.md")).unwrap(),
            b"---\nmax_turns: 10\nallowed_tools: [Read, Glob, Grep, Skill]\n---\n\nTODO: describe what the agent should do\n"
        );
        assert_eq!(
            fs::read(suite.join("graders").join("criteria.md")).unwrap(),
            b"---\ntype: llm\nweight: 1\n---\n\nTODO: describe what a successful response looks like\n"
        );
        assert!(write_bare_template(&suite, "smoke").is_err());
        assert!(eval_suite_dir(temp.path(), DEFAULT_EVAL_DIR, "../escape").is_err());
    }

    #[test]
    fn empty_aggregate_uses_v1_camel_case_schema() {
        let result = AggregateResult {
            schema_version: 1,
            claude_version: "0.12.0".to_string(),
            started_at: "2026-01-01T00:00:00.000Z".to_string(),
            duration_seconds: 0.0,
            cost_usd: 0.0,
            partial: false,
            suite: EvalSuiteResult {
                root: PathBuf::from("/tmp/plugin"),
                ablation: "none".to_string(),
                plugin_id: None,
                model_override: None,
                judge_model: "haiku".to_string(),
                case_filter: None,
                tag_filters: Vec::new(),
                threshold: 1.0,
                plugins: Vec::new(),
            },
            cases: Vec::new(),
            aggregates: EvalAggregates {
                cases_total: 0,
                cases_passed: 0,
                overall_score: 0.0,
                overall_pass_rate: 0.0,
            },
        };
        let json = serde_json::to_value(result).unwrap();
        assert_eq!(json["schemaVersion"], 1);
        assert_eq!(json["durationSeconds"], 0.0);
        assert_eq!(json["costUsd"], 0.0);
        assert_eq!(json["suite"]["tagFilters"], serde_json::json!([]));
        assert_eq!(json["aggregates"]["casesTotal"], 0);
        assert_eq!(json["aggregates"]["overallPassRate"], 0.0);
        assert_eq!(json["cases"], serde_json::json!([]));
    }

    #[test]
    fn free_graders_and_globs_are_deterministic() {
        assert!(glob_matches("smoke-*", "smoke-one"));
        assert!(!glob_matches("smoke-?", "smoke-long"));
        let definitions = vec![GraderDefinition::Regex {
            name: "regex".to_string(),
            pattern: "ok".to_string(),
            weight: 2.0,
        }];
        let results = vec![GraderResult {
            name: "regex".to_string(),
            score: Some(1.0),
            passed: true,
            reason: String::new(),
            skipped: false,
        }];
        assert_eq!(weighted_score(&results, &definitions, true), 1.0);
    }

    #[test]
    fn html_report_escapes_plugin_controlled_text() {
        let malicious_run = EvalRunResult {
            arm: "<with>".to_string(),
            run: 1,
            success: false,
            passed: false,
            output: "<script>output()</script>".to_string(),
            cost_usd: 0.0,
            judge_cost_usd: 0.0,
            turns: 0,
            duration_seconds: 0.0,
            started_at: "2026-01-01T00:00:00.000Z".to_string(),
            trace_path: None,
            skipped_paid_graders: false,
            graders: vec![GraderResult {
                name: "<grader>".to_string(),
                score: Some(0.0),
                passed: false,
                reason: "<img src=x onerror=y>".to_string(),
                skipped: false,
            }],
            score: 0.0,
            error: Some("<b>error</b>".to_string()),
        };
        let result = AggregateResult {
            schema_version: 1,
            claude_version: "0.12.0".to_string(),
            started_at: "2026-01-01T00:00:00.000Z".to_string(),
            duration_seconds: 0.0,
            cost_usd: 0.0,
            partial: false,
            suite: EvalSuiteResult {
                root: PathBuf::from("."),
                ablation: "none".to_string(),
                plugin_id: Some("<script>alert(1)</script>".to_string()),
                model_override: None,
                judge_model: "haiku".to_string(),
                case_filter: None,
                tag_filters: Vec::new(),
                threshold: 1.0,
                plugins: Vec::new(),
            },
            cases: vec![EvalResult {
                name: "<img onerror=x>".to_string(),
                dir: PathBuf::from("."),
                source: PathBuf::from("prompt.md"),
                prompt_markdown: "<iframe>prompt</iframe>".to_string(),
                model: None,
                runs_per_case: 1,
                timeout_seconds: 600,
                max_turns: 10,
                tags: Vec::new(),
                graders: Vec::new(),
                arms: EvalArms {
                    with_plugin: vec![malicious_run],
                    without_plugin: None,
                },
                aggregates: EvalCaseAggregates {
                    score: 1.0,
                    pass_rate: 1.0,
                    score_without: None,
                    pass_rate_without: None,
                    delta: None,
                },
            }],
            aggregates: EvalAggregates {
                cases_total: 1,
                cases_passed: 1,
                overall_score: 1.0,
                overall_pass_rate: 1.0,
            },
        };
        let html = render_html_report(&result);
        assert!(!html.contains("<script>alert"));
        assert!(!html.contains("<img onerror"));
        assert!(!html.contains("<script>output"));
        assert!(!html.contains("<iframe>prompt"));
        assert!(!html.contains("<b>error"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("&lt;iframe&gt;"));
        assert!(html.contains("&lt;grader&gt;"));
    }

    #[cfg(unix)]
    #[test]
    fn discovery_rejects_symlinked_case_files() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let evals = temp.path().join("evals").join("case");
        fs::create_dir_all(&evals).unwrap();
        let outside = temp.path().join("outside.md");
        fs::write(&outside, "secret").unwrap();
        symlink(&outside, evals.join("prompt.md")).unwrap();
        let error = discover_cases(temp.path(), DEFAULT_EVAL_DIR).unwrap_err();
        assert!(error.contains("symlink"));
    }
}
