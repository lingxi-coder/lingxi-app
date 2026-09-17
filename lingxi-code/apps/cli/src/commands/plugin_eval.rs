//! `lingxi-cli plugin eval` — isolated, scored plugin evaluation.
//!
//! The public command surface mirrors Claude Code 2.1.220. Evaluation turns
//! are deliberately launched through the current CLI executable instead of a
//! private orchestrator shortcut: plugin loading, managed deny rules,
//! permission policy, sandboxing, hooks, model selection, and cost accounting
//! therefore use the same production path as an ordinary print-mode turn.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{IsTerminal as _, Write as _};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use clap::{Args, Subcommand};
use serde::ser::SerializeStruct as _;
use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value as YamlValue};

use crate::exit_codes::{RUNTIME_ERROR, SUCCESS};

const PARTIAL_EXIT: i32 = 2;
const RESULT_SCHEMA_VERSION: u32 = 1;
const DEFAULT_RUNS: u32 = 3;
const DEFAULT_MAX_TURNS: u32 = 20;
const DEFAULT_TIMEOUT_SECONDS: u64 = 300;
const JSON_STDOUT_SENTINEL: &str = "\u{1f}stdout";
const MAX_CASE_FILES: usize = 1_000;
const MAX_STAGE_FILES: usize = 20_000;
const MAX_STAGE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_PROMPT_BYTES: usize = 1_048_576;
const MAX_GRADER_OUTPUT_BYTES: usize = 262_144;
const CLAUDE_PLUGIN_MANIFEST_DIR: &str = ".claude-plugin";

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
        default_missing_value = JSON_STDOUT_SENTINEL
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
    #[arg(long = "max-cost-usd", value_name = "usd", value_parser = parse_non_negative_f64)]
    pub max_cost_usd: Option<f64>,

    /// Mock stand-ins for MCP servers, from <eval dir>/mocks/ (record | off;
    /// default: record — off spawns the real servers, gated by --allow-tools
    /// as usual).
    #[arg(
        long,
        value_name = "mode",
        value_parser = ["record", "off"],
        default_value = "record"
    )]
    pub mocks: String,

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
    /// Author-declared arms, run in order against one shared grader set.
    ///
    /// Empty is the ordinary single-arm case, or the two-arm `with-without`
    /// ablation. Two or more entries replace both: the FIRST arm is the
    /// baseline every other arm is compared against, and the LAST is the one
    /// the case's headline score reports.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arms: Vec<EvalArm>,
}

/// One author-declared arm of a case.
///
/// An arm changes only what the agent is asked and what settings it runs
/// under. Everything else -- graders, model, turn and timeout limits, tool
/// surface, scaffold -- stays shared, which is what makes the runs paired and
/// their difference attributable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalArm {
    /// Stable arm label. Used in the report, in run directory names, and in
    /// the delta table, so it must be unique within a case.
    pub label: String,
    /// Prompt for this arm. Absent means the case prompt verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Settings JSON merged into the child agent's `--settings` object.
    /// Later keys win over the harness's own plugin-enablement keys, so an
    /// arm can turn a subsystem on for itself alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<serde_json::Value>,
}

/// A free deterministic grader or an LLM rubric grader.
#[derive(Debug, Clone, Deserialize)]
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

impl Serialize for GraderDefinition {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut state = serializer.serialize_struct("GraderDefinition", 5)?;
        state.serialize_field("name", self.name())?;
        let (kind, markdown, config) = match self {
            Self::Regex { pattern, .. } => {
                ("regex", None, serde_json::json!({ "pattern": pattern }))
            }
            Self::ToolOrder { tools, .. } => {
                ("tool_order", None, serde_json::json!({ "tools": tools }))
            }
            Self::ToolUsed { tool, .. } => ("tool_used", None, serde_json::json!({ "tool": tool })),
            Self::FileExists { path, .. } => {
                ("file_exists", None, serde_json::json!({ "path": path }))
            }
            Self::Llm { rubric, .. } => (
                "llm",
                Some(rubric.as_str()),
                serde_json::json!({ "criteria": rubric, "focus": "last_message" }),
            ),
            Self::Baseline { rubric, .. } => (
                "baseline",
                Some(rubric.as_str()),
                serde_json::json!({ "criteria": rubric, "focus": "last_message" }),
            ),
        };
        state.serialize_field("type", kind)?;
        state.serialize_field("weight", &self.weight())?;
        if let Some(markdown) = markdown {
            state.serialize_field("graderMarkdown", markdown)?;
        }
        state.serialize_field("config", &config)?;
        state.end()
    }
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
#[derive(Debug, Clone, Deserialize)]
pub struct GraderResult {
    /// Grader name.
    pub name: String,
    /// Normalized score in `[0, 1]`, or absent when skipped.
    pub score: Option<f64>,
    /// Whether the grader met its full-credit threshold.
    pub passed: bool,
    /// Configured grader weight.
    pub weight: f64,
    /// Human-readable reason.
    pub reason: String,
    /// Whether budget policy skipped this grader.
    pub skipped: bool,
    /// Whether this grader applies only to the with-plugin arm.
    pub with_only: bool,
    /// Whether the grader contributes to the score.
    pub scored: bool,
}

impl Serialize for GraderResult {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut state = serializer.serialize_struct("GraderResult", 7)?;
        state.serialize_field("name", &self.name)?;
        state.serialize_field("passed", &self.passed)?;
        state.serialize_field("weight", &self.weight)?;
        if let Some(score) = self.score {
            state.serialize_field("score", &score)?;
        }
        state.serialize_field("explanation", &self.reason)?;
        state.serialize_field("withOnly", &self.with_only)?;
        state.serialize_field("scored", &self.scored)?;
        state.end()
    }
}

impl GraderResult {
    fn from_definition(
        definition: &GraderDefinition,
        score: Option<f64>,
        passed: bool,
        reason: String,
        skipped: bool,
    ) -> Self {
        Self {
            name: definition.name().to_string(),
            score,
            passed,
            weight: definition.weight(),
            reason,
            skipped,
            with_only: false,
            scored: !skipped,
        }
    }
}

/// One isolated agent run and its graders.
#[derive(Debug, Clone, Deserialize)]
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
    /// Mock expectation that deliberately stopped scoring this run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aborted: Option<EvalMockAborted>,
    /// Mock-server metadata for `--mocks record` with-plugin arms.
    pub mocks: Option<EvalMocks>,
}

impl Serialize for EvalRunResult {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut state = serializer.serialize_struct("EvalRunResult", 13)?;
        state.serialize_field("score", &self.score)?;
        state.serialize_field("passed", &self.passed)?;
        state.serialize_field("turns", &self.turns)?;
        state.serialize_field("costUsd", &self.cost_usd)?;
        state.serialize_field("judgeCostUsd", &self.judge_cost_usd)?;
        state.serialize_field("durationSeconds", &self.duration_seconds)?;
        state.serialize_field("startedAt", &self.started_at)?;
        if let Some(error) = &self.error {
            state.serialize_field("error", error)?;
        }
        if let Some(aborted) = &self.aborted {
            state.serialize_field("aborted", aborted)?;
        }
        if let Some(path) = &self.trace_path {
            state.serialize_field("tracePath", path)?;
        }
        state.serialize_field("skippedPaidGraders", &self.skipped_paid_graders)?;
        if let Some(mocks) = &self.mocks {
            state.serialize_field("mocks", mocks)?;
        }
        state.serialize_field("graders", &self.graders)?;
        state.end()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Mock-server metadata captured for one with-plugin run.
pub struct EvalMocks {
    /// Mock servers registered for the run.
    pub servers: Vec<EvalMockServer>,
    /// Non-fatal mock configuration warnings.
    pub warnings: Vec<String>,
    /// Aggregate mock call counters.
    pub calls: EvalMockCalls,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// One mock MCP server exposed to the evaluated plugin.
pub struct EvalMockServer {
    /// MCP server name.
    pub server: String,
    /// Server kind (`shadow` or `standalone`).
    pub kind: String,
    /// Mocked tools on the server.
    pub tools: Vec<EvalMockTool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// One mocked MCP tool.
pub struct EvalMockTool {
    /// Tool name.
    pub tool: String,
    /// Responder kind.
    pub responder: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Call counters emitted by the mock runtime.
pub struct EvalMockCalls {
    /// Total mock calls.
    pub total: u64,
    /// Calls that returned an error.
    pub errors: u64,
    /// Calls attempted against unmocked tools.
    pub unmocked: Vec<EvalMockUnmocked>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Count of calls attempted against one unmocked tool.
pub struct EvalMockUnmocked {
    /// Tool name.
    pub tool: String,
    /// Number of rejected calls.
    pub count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Mock expectation that aborted one eval run.
pub struct EvalMockAborted {
    /// Mock server directory name.
    pub server: String,
    /// Mock tool name.
    pub tool: String,
    /// Stable human-readable abort reason.
    pub reason: String,
}

/// Runs grouped by arm label, in the order the arms ran.
///
/// Serializes as a JSON object keyed by label, so an ablation suite still
/// reports the familiar `with` / `without` keys and an arms suite reports its
/// own. Order is meaningful: the first entry is the baseline.
#[derive(Debug, Clone, Default)]
pub struct EvalArms {
    /// Label and its runs, baseline first.
    pub by_label: Vec<(String, Vec<EvalRunResult>)>,
}

impl EvalArms {
    /// Every run across every arm.
    pub fn runs(&self) -> impl Iterator<Item = &EvalRunResult> {
        self.by_label.iter().flat_map(|(_, runs)| runs.iter())
    }

    /// Runs for one label, if that arm ran.
    #[must_use]
    pub fn labelled(&self, label: &str) -> Option<&[EvalRunResult]> {
        self.by_label
            .iter()
            .find(|(name, _)| name == label)
            .map(|(_, runs)| runs.as_slice())
    }
}

impl Serialize for EvalArms {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(self.by_label.len()))?;
        for (label, runs) in &self.by_label {
            map.serialize_entry(label, runs)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for EvalArms {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ArmsVisitor;
        impl<'de> serde::de::Visitor<'de> for ArmsVisitor {
            type Value = EvalArms;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a map of arm label to runs")
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut access: M,
            ) -> Result<EvalArms, M::Error> {
                // Encounter order, not sorted order: the first arm is the
                // baseline and the report has to keep saying which one that is.
                let mut by_label = Vec::new();
                while let Some((label, runs)) = access.next_entry::<String, Vec<EvalRunResult>>()? {
                    by_label.push((label, runs));
                }
                Ok(EvalArms { by_label })
            }
        }
        deserializer.deserialize_map(ArmsVisitor)
    }
}

/// Aggregate score block for one case.
///
/// With two or more arms the headline figures describe the LAST arm and the
/// `_without` figures describe the FIRST, which is what makes the ablation
/// wording still read correctly: its arms are ordered `without` then `with`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalCaseAggregates {
    /// Final arm's score.
    pub score: f64,
    /// Final arm's pass rate.
    pub pass_rate: f64,
    /// Baseline arm's score.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score_without: Option<f64>,
    /// Baseline arm's pass rate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pass_rate_without: Option<f64>,
    /// Final minus baseline score.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta: Option<f64>,
    /// Every arm's own score, baseline first. Present only when the case
    /// declared its own arms; two-arm ablation is fully described above.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub per_arm: Vec<EvalArmAggregate>,
}

/// One arm's score beside its distance from the baseline arm.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalArmAggregate {
    /// Arm label.
    pub label: String,
    /// Mean grader score across this arm's runs.
    pub score: f64,
    /// Fraction of this arm's runs that passed.
    pub pass_rate: f64,
    /// Score minus the baseline arm's score. `None` on the baseline itself.
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
    pub source: String,
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
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
    /// Manifest version.
    pub version: String,
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
    #[serde(skip_serializing_if = "is_default_judge_model")]
    pub judge_model: String,
    /// Name filter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub case_filter: Option<String>,
    /// Tag filters.
    #[serde(skip_serializing_if = "Vec::is_empty")]
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
    /// Mean with-minus-without score delta.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mean_delta: Option<f64>,
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
    /// Stable reason for a partial result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partial_reason: Option<String>,
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
    if let Some(path) = cli.json.as_deref() {
        if path != JSON_STDOUT_SENTINEL && !path.ends_with(".json") {
            eprintln!(
                "Error: --json output path must end in .json (got '{path}'). If that is your eval target, put it before --json."
            );
            return RUNTIME_ERROR;
        }
    }
    // `plugin eval` GRADUATED out of early access. 2.1.220 gated it on
    // `CLAUDE_CODE_WALNUT_SPIRE` (still pinned by that release's fixture); in
    // 2.1.270 neither the env name nor the "currently in early access" refusal
    // appears anywhere in the binary, while the command itself is fully
    // documented with help text and examples. Keeping the gate made the whole
    // subcommand unreachable unless the user happened to set a variable that
    // upstream no longer reads.
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
            if result.cases.is_empty() && !result.partial {
                let eval_dir = resolve_eval_dir(cli.eval_dir.as_deref(), &result.suite.root)
                    .unwrap_or_else(|_| DEFAULT_EVAL_DIR.to_string());
                if cli.json.is_some() {
                    eprintln!("No eval cases found under {}.", result.suite.root.display());
                    if let Err(error) = emit_empty_json(cli, &result, cwd) {
                        eprintln!("lingxi-cli plugin eval: {error}");
                        return PARTIAL_EXIT;
                    }
                } else {
                    eprintln!(
                        "{}",
                        no_eval_cases_message(
                            &result.suite.root,
                            &eval_dir,
                            cli.eval_dir.as_deref(),
                        )
                    );
                }
                return RUNTIME_ERROR;
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

fn no_eval_cases_message(root: &Path, eval_dir: &str, flag: Option<&str>) -> String {
    let manifest = plugin_manifest_path(root);
    let source = if flag.is_some() {
        "from --eval-dir".to_string()
    } else if manifest_eval_dir(root).is_some() {
        format!("from {}; pass --eval-dir to override", manifest.display())
    } else {
        "the default".to_string()
    };
    let eval_dir_flag = flag.map(|_| format!(" --eval-dir {eval_dir}"));
    let eval_dir_flag = eval_dir_flag.as_deref().unwrap_or_default();
    format!(
        "No eval cases found under {}.\nCases are expected in a {eval_dir}/ directory under {} ({source}), each case a directory containing case.yaml or prompt.md.\nRun `claude plugin eval init{eval_dir_flag}` for a guided interview, or `claude plugin eval init --bare <name>{eval_dir_flag}` to scaffold a blank case.",
        root.display(),
        root.display()
    )
}

fn is_default_judge_model(model: &String) -> bool {
    model == "haiku"
}

fn emit_empty_json(cli: &Cli, result: &AggregateResult, cwd: &Path) -> Result<(), String> {
    let mut json = serialize_result_pretty(result)?;
    json.push('\n');
    match cli.json.as_deref() {
        Some(JSON_STDOUT_SENTINEL) => print!("{json}"),
        Some(path) => {
            atomic_write(&resolve_output_path(cwd, Path::new(path)), json.as_bytes())?;
            println!("Wrote {path}");
        }
        None => {}
    }
    Ok(())
}

fn serialize_result_pretty(result: &AggregateResult) -> Result<String, String> {
    let mut value = serde_json::to_value(result)
        .map_err(|error| format!("failed to serialize eval result: {error}"))?;
    normalize_js_numbers(&mut value);
    serde_json::to_string_pretty(&value)
        .map_err(|error| format!("failed to serialize eval result: {error}"))
}

fn normalize_js_numbers(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Array(values) => {
            values.iter_mut().for_each(normalize_js_numbers);
        }
        serde_json::Value::Object(values) => {
            values.values_mut().for_each(normalize_js_numbers);
        }
        serde_json::Value::Number(number) => {
            const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;
            if number.is_f64() {
                if let Some(number) = number.as_f64() {
                    if number.is_finite()
                        && number.fract() == 0.0
                        && number.abs() <= MAX_SAFE_INTEGER
                    {
                        *value = serde_json::Value::Number(serde_json::Number::from(number as i64));
                    }
                }
            }
        }
        _ => {}
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
    if cli.eval_dir.is_none() {
        if let Some(warning) = manifest_eval_setting(&target.root).warning {
            eprintln!("{warning}");
        }
    }
    let eval_dir = resolve_eval_dir(cli.eval_dir.as_deref(), &target.root)?;
    if cli.json.is_none() && cli.eval_dir.is_none() && manifest_eval_dir(&target.root).is_some() {
        eprintln!(
            "Using eval directory {eval_dir}/ from {}",
            plugin_manifest_path(&target.root).display()
        );
    }
    let mut cases = discover_cases(&target.root, &eval_dir)?;
    cases.retain(|case| selected_case(case, cli));

    let plugin_manifest = plugin_manifest_path(&target.root);
    let plugin_resolved = target.kind == TargetKind::Named || plugin_manifest.is_file();
    let ablation = cli.ablation.clone().unwrap_or_else(|| {
        if !cases.is_empty() && plugin_resolved {
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
        plugins: if !cases.is_empty() && plugin_resolved {
            let (name, version) = plugin_manifest_identity(&target.root)
                .unwrap_or_else(|| (target.original.clone(), "unknown".to_string()));
            vec![EvalSuitePlugin {
                name,
                version,
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
                claude_version: platform_api::CLAUDE_CODE_VERSION.to_string(),
                started_at,
                duration_seconds: 0.0,
                cost_usd: 0.0,
                partial: false,
                partial_reason: None,
                suite,
                cases: Vec::new(),
                aggregates: EvalAggregates {
                    cases_total: 0,
                    cases_passed: 0,
                    overall_score: 0.0,
                    overall_pass_rate: 0.0,
                    mean_delta: None,
                },
            },
            RUNTIME_ERROR,
        ));
    }
    if cli.ablation.is_none() && plugin_resolved {
        eprintln!(
            "Ablation: defaulting to with-without — a plugin resolved from this path, so each case also runs a no-plugin baseline arm (2× runs) and reports Δ; graders marked with-only (including `tool_used: Skill`) become a plugin-fired indicator rather than part of the score. Pass --ablation none for the previous single-arm run and scoring."
        );
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
    'case_loop: for case in cases {
        let run_count = cli.runs.or(case.runs).unwrap_or(DEFAULT_RUNS);
        // Both shapes reduce to the same thing: an ordered list of arms with
        // the baseline first. The ablation is the implicit two-arm plan; a
        // case that declares `arms:` supplies its own.
        let plan = arm_plan(&case, &ablation);
        let mut runs: Vec<EvalRunResult> = Vec::new();
        for run_number in 1..=run_count {
            if budget_reached(cli.max_cost_usd, total_cost) {
                partial = true;
                break;
            }
            let mut baseline_output: Option<String> = None;
            for (index, arm) in plan.iter().enumerate() {
                if index > 0 && budget_reached(cli.max_cost_usd, total_cost) {
                    partial = true;
                    break;
                }
                let result = run_one_arm(
                    cli,
                    &target.root,
                    &target.disabled_plugin_ids,
                    &case,
                    arm,
                    run_number,
                    &temp.path,
                    total_cost,
                    baseline_output.as_deref(),
                    index == 0,
                )
                .await;
                total_cost += result.cost_usd;
                partial |= result.skipped_paid_graders;
                if index == 0 && plan.len() > 1 {
                    baseline_output = Some(result.output.clone());
                }
                runs.push(result);
            }
        }
        if runs.is_empty() && budget_reached(cli.max_cost_usd, total_cost) {
            partial = true;
            break 'case_loop;
        }
        let mut by_label: Vec<(String, Vec<EvalRunResult>)> = plan
            .iter()
            .map(|arm| (arm.label.clone(), Vec::new()))
            .collect();
        for run in runs {
            if let Some((_, bucket)) = by_label.iter_mut().find(|(label, _)| *label == run.arm) {
                bucket.push(run);
            }
        }
        let baseline = by_label.first().map(|(_, runs)| run_average(runs));
        let baseline_pass = by_label.first().map(|(_, runs)| run_pass_rate(runs));
        let (score, pass_rate) = by_label.last().map_or((0.0, 0.0), |(_, runs)| {
            (run_average(runs), run_pass_rate(runs))
        });
        let paired = plan.len() > 1;
        let per_arm = if case.arms.is_empty() {
            Vec::new()
        } else {
            by_label
                .iter()
                .enumerate()
                .map(|(index, (label, runs))| EvalArmAggregate {
                    label: label.clone(),
                    score: run_average(runs),
                    pass_rate: run_pass_rate(runs),
                    delta: (index > 0)
                        .then(|| baseline.map(|base| run_average(runs) - base))
                        .flatten(),
                })
                .collect()
        };
        let score_without = paired.then_some(baseline).flatten();
        let case_dir = case.source.parent().unwrap_or(&target.root);
        let case_dir = case_dir
            .strip_prefix(&target.root)
            .unwrap_or(case_dir)
            .to_path_buf();
        let case_source =
            if case.source.file_name().and_then(|name| name.to_str()) == Some("case.yaml") {
                "yaml"
            } else {
                "prose"
            }
            .to_string();
        results.push(EvalResult {
            name: case.name,
            dir: case_dir,
            source: case_source,
            prompt_markdown: case.prompt,
            model: cli.model.clone().or(case.model),
            runs_per_case: case.runs.unwrap_or(DEFAULT_RUNS),
            timeout_seconds: case.timeout_seconds.unwrap_or(DEFAULT_TIMEOUT_SECONDS),
            max_turns: case.max_turns.unwrap_or(DEFAULT_MAX_TURNS),
            tags: case.tags,
            graders: case.graders,
            arms: EvalArms { by_label },
            aggregates: EvalCaseAggregates {
                score,
                pass_rate,
                score_without,
                pass_rate_without: paired.then_some(baseline_pass).flatten(),
                delta: score_without.map(|base| score - base),
                per_arm,
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
    let pass_rate = if cases_total == 0 {
        0.0
    } else {
        cases_passed as f64 / cases_total as f64
    };
    let deltas = results
        .iter()
        .filter_map(|case| case.aggregates.delta)
        .collect::<Vec<_>>();
    let mean_delta = (!deltas.is_empty()).then(|| deltas.iter().sum::<f64>() / deltas.len() as f64);
    let run_failed = results.iter().any(|case| {
        case.arms
            .runs()
            .any(|run| run.error.is_some() || run.aborted.is_some())
    });
    let threshold_failed = cases_passed != cases_total || run_failed;
    let auth_failed = results
        .iter()
        .flat_map(|case| case.arms.runs())
        .filter_map(|run| run.error.as_deref())
        .any(|error| error.contains("Not logged in"));
    let cost_ceiling = budget_reached(cli.max_cost_usd, total_cost);
    partial |= auth_failed || cost_ceiling;
    let partial_reason = if auth_failed {
        Some("auth_failed".to_string())
    } else if cost_ceiling {
        Some("cost_ceiling".to_string())
    } else {
        None
    };
    let exit = if partial {
        PARTIAL_EXIT
    } else if threshold_failed {
        RUNTIME_ERROR
    } else {
        SUCCESS
    };
    Ok((
        AggregateResult {
            schema_version: RESULT_SCHEMA_VERSION,
            claude_version: platform_api::CLAUDE_CODE_VERSION.to_string(),
            started_at,
            duration_seconds: started.elapsed().as_secs_f64(),
            cost_usd: total_cost,
            partial,
            partial_reason,
            suite,
            cases: results,
            aggregates: EvalAggregates {
                cases_total,
                cases_passed,
                overall_score: score,
                overall_pass_rate: pass_rate,
                mean_delta,
            },
        },
        exit,
    ))
}

#[allow(clippy::too_many_arguments)]
/// One arm resolved against its case: what to ask, under which settings, and
/// whether the target plugin is loaded for it.
struct PlannedArm {
    label: String,
    prompt: String,
    plugin_enabled: bool,
    settings: Option<serde_json::Value>,
}

/// Reduce a case to the ordered arms it will actually run, baseline first.
fn arm_plan(case: &EvalCase, ablation: &str) -> Vec<PlannedArm> {
    if !case.arms.is_empty() {
        // Author-declared arms replace the ablation pair: every arm gets the
        // resolved target, and what differs between them is exactly what the
        // case wrote down.
        return case
            .arms
            .iter()
            .map(|arm| PlannedArm {
                label: arm.label.clone(),
                prompt: arm.prompt.clone().unwrap_or_else(|| case.prompt.clone()),
                plugin_enabled: true,
                settings: arm.settings.clone(),
            })
            .collect();
    }
    let with = PlannedArm {
        label: "with".to_string(),
        prompt: case.prompt.clone(),
        plugin_enabled: true,
        settings: None,
    };
    if ablation != "with-without" {
        return vec![with];
    }
    vec![
        PlannedArm {
            label: "without".to_string(),
            prompt: case.prompt.clone(),
            plugin_enabled: false,
            settings: None,
        },
        with,
    ]
}

async fn run_one_arm(
    cli: &Cli,
    plugin_root: &Path,
    disabled_plugin_ids: &[String],
    case: &EvalCase,
    planned: &PlannedArm,
    run_number: u32,
    temp_root: &Path,
    prior_cost: f64,
    baseline_output: Option<&str>,
    is_baseline: bool,
) -> EvalRunResult {
    let arm = planned.label.as_str();
    let plugin_enabled = planned.plugin_enabled;
    let started_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let discovered_mocks = if plugin_enabled && cli.mocks == "record" {
        match discover_eval_mocks(cli, plugin_root, case) {
            Ok(mocks) => mocks,
            Err(error) => return failed_run(arm, run_number, error),
        }
    } else {
        None
    };
    let run_dir = temp_root
        .join(safe_segment(&case.name))
        .join(format!("{}-{run_number}", safe_segment(arm)));
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

    let prepared_mocks = match discovered_mocks.as_ref() {
        Some(mocks) => match prepare_mock_runtime(plugin_root, &run_dir, mocks) {
            Ok(runtime) => Some(runtime),
            Err(error) => return failed_run(arm, run_number, error),
        },
        None => None,
    };
    let prepared_plugin = if plugin_enabled && prepared_mocks.is_none() {
        match prepare_eval_plugin(plugin_root) {
            Ok(plugin) => plugin,
            Err(error) => return failed_run(arm, run_number, error),
        }
    } else {
        None
    };

    let model = cli.model.as_deref().or(case.model.as_deref());
    let mut allowed_tools = case.allowed_tools.clone();
    for tool in &cli.allow_tools {
        if !allowed_tools.contains(tool) {
            allowed_tools.push(tool.clone());
        }
    }
    if let Some(runtime) = &prepared_mocks {
        for tool in &runtime.mocked_tools {
            if !allowed_tools.contains(tool) {
                allowed_tools.push(tool.clone());
            }
        }
    }
    let plugin_for_agent = prepared_mocks
        .as_ref()
        .map(|runtime| runtime.plugin_root.as_path())
        .or_else(|| {
            prepared_plugin
                .as_ref()
                .map(|plugin| plugin.plugin_root.as_path())
        })
        .or_else(|| plugin_enabled.then_some(plugin_root));
    let mut output = run_agent(
        &planned.prompt,
        model,
        &allowed_tools,
        plugin_for_agent,
        disabled_plugin_ids,
        &run_dir,
        case.max_turns.unwrap_or(DEFAULT_MAX_TURNS),
        case.timeout_seconds.unwrap_or(DEFAULT_TIMEOUT_SECONDS),
        cli.verbose,
        case.append_system_prompt.as_deref(),
        prepared_mocks.as_ref(),
        planned.settings.as_ref(),
    )
    .await;
    let mut mocks = discovered_mocks.map(|mocks| mocks.report);
    let mut aborted = None;
    if let (Some(runtime), Some(report)) = (&prepared_mocks, &mut mocks) {
        match aggregate_mock_calls(&runtime.call_logs) {
            Ok((calls, mock_abort)) => {
                let observed_mock_tool = output
                    .tools_used
                    .iter()
                    .any(|tool| runtime.mocked_tools.contains(tool));
                if observed_mock_tool && calls.total == 0 {
                    output.success = false;
                    output.error = Some(
                        "mocked tool calls appeared in the agent trace without a matching stand-in record"
                            .to_string(),
                    );
                }
                report.calls = calls;
                aborted = mock_abort;
            }
            Err(error) => {
                output.success = false;
                output.error = Some(error);
            }
        }
    }
    let mut run_cost = output.cost_usd;
    let mut judge_cost = 0.0;
    let mut grader_results = Vec::new();
    for grader in &case.graders {
        if aborted.is_some() {
            break;
        }
        if matches!(grader, GraderDefinition::Baseline { .. }) && baseline_output.is_none() {
            // The baseline arm has nothing to compare against by definition.
            if is_baseline {
                continue;
            }
            grader_results.push(GraderResult::from_definition(
                grader,
                Some(0.0),
                false,
                "baseline grader requires --ablation with-without or a case with arms".to_string(),
                false,
            ));
            continue;
        }
        if grader.is_paid() && budget_reached(cli.max_cost_usd, prior_cost + run_cost) {
            grader_results.push(GraderResult::from_definition(
                grader,
                None,
                false,
                "skipped after cost ceiling was reached".to_string(),
                true,
            ));
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
    let score = if aborted.is_some() {
        0.0
    } else {
        weighted_score(&grader_results, &case.graders, output.success)
    };
    let skipped_paid_graders = grader_results.iter().any(|result| result.skipped);
    EvalRunResult {
        arm: arm.to_string(),
        run: run_number,
        success: output.success && aborted.is_none(),
        passed: output.success && aborted.is_none() && score >= 1.0,
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
        aborted,
        mocks,
    }
}

#[derive(Default)]
struct MockServerAccumulator {
    tools: BTreeMap<String, MockResponderAccumulator>,
    described_tools: BTreeMap<String, serde_json::Value>,
}

struct MockResponderAccumulator {
    kind: String,
    body: String,
    base_dir: PathBuf,
    error: bool,
    expect: Option<serde_json::Value>,
}

struct DiscoveredEvalMocks {
    report: EvalMocks,
    runtime: Vec<RuntimeMockServer>,
}

struct RuntimeMockServer {
    registered_name: String,
    spec: crate::commands::plugin_eval_mock::MockServerSpec,
}

struct PreparedMockRuntime {
    _staging: RunTemp,
    plugin_root: PathBuf,
    mcp_config: String,
    call_logs: Vec<PathBuf>,
    mocked_tools: Vec<String>,
}

struct PreparedEvalPlugin {
    _staging: RunTemp,
    plugin_root: PathBuf,
}

fn prepare_mock_runtime(
    plugin_root: &Path,
    run_dir: &Path,
    discovered: &DiscoveredEvalMocks,
) -> Result<PreparedMockRuntime, String> {
    let staging = RunTemp::create(false)?;
    let staged_plugin = staging.path.join("plugin");
    copy_plugin_snapshot(plugin_root, &staged_plugin)?;
    adapt_staged_plugin_manifest(&staged_plugin)?;
    strip_staged_plugin_mcp(&staged_plugin)?;

    let executable = std::env::current_exe()
        .map_err(|error| format!("failed to resolve current executable: {error}"))?;
    let runtime_dir = run_dir.join("mock-runtime");
    fs::create_dir_all(&runtime_dir).map_err(|error| {
        format!(
            "failed to create mock runtime directory {}: {error}",
            runtime_dir.display()
        )
    })?;
    set_private_dir_permissions(&runtime_dir)?;

    let mut configs = serde_json::Map::new();
    let mut call_logs = Vec::new();
    let mut mocked_tools = Vec::new();
    for (index, server) in discovered.runtime.iter().enumerate() {
        let spec_path = runtime_dir.join(format!("server-{index}.json"));
        let call_log = runtime_dir.join(format!("calls-{index}.jsonl"));
        let bytes = serde_json::to_vec(&server.spec)
            .map_err(|error| format!("failed to serialize mock server spec: {error}"))?;
        atomic_write(&spec_path, &bytes)?;
        let launch_env = serde_json::Map::from_iter([
            (
                crate::commands::plugin_eval_mock::SPEC_ENV.to_string(),
                serde_json::json!(spec_path.to_string_lossy()),
            ),
            (
                crate::commands::plugin_eval_mock::CALLS_ENV.to_string(),
                serde_json::json!(call_log.to_string_lossy()),
            ),
        ]);
        configs.insert(
            server.registered_name.clone(),
            serde_json::json!({
                "type": "stdio",
                "command": executable.to_string_lossy(),
                "args": [],
                "env": launch_env,
            }),
        );
        let normalized_server = protocol::normalize_name_for_mcp(&server.registered_name);
        mocked_tools.extend(server.spec.tools.iter().map(|tool| {
            format!(
                "mcp__{normalized_server}__{}",
                protocol::normalize_name_for_mcp(&tool.name)
            )
        }));
        call_logs.push(call_log);
    }
    Ok(PreparedMockRuntime {
        _staging: staging,
        plugin_root: staged_plugin,
        mcp_config: serde_json::json!({ "mcpServers": configs }).to_string(),
        call_logs,
        mocked_tools,
    })
}

fn prepare_eval_plugin(plugin_root: &Path) -> Result<Option<PreparedEvalPlugin>, String> {
    let parity_manifest = plugin_manifest_path(plugin_root);
    if !parity_manifest.is_file() {
        return Ok(None);
    }
    let staging = RunTemp::create(false)?;
    let staged_plugin = staging.path.join("plugin");
    copy_plugin_snapshot(plugin_root, &staged_plugin)?;
    adapt_staged_plugin_manifest(&staged_plugin)?;
    Ok(Some(PreparedEvalPlugin {
        _staging: staging,
        plugin_root: staged_plugin,
    }))
}

fn adapt_staged_plugin_manifest(plugin_root: &Path) -> Result<(), String> {
    let source = plugin_manifest_path(plugin_root);
    if !source.is_file() {
        return Ok(());
    }
    let destination = plugin_root
        .join(branding::PLUGIN_MANIFEST_DIR)
        .join("plugin.json");
    let bytes = fs::read(&source)
        .map_err(|error| format!("failed to read {}: {error}", source.display()))?;
    atomic_write(&destination, &bytes)
}

fn strip_staged_plugin_mcp(plugin_root: &Path) -> Result<(), String> {
    let root_mcp = plugin_root.join(".mcp.json");
    if root_mcp.is_file() {
        fs::remove_file(&root_mcp)
            .map_err(|error| format!("failed to remove {}: {error}", root_mcp.display()))?;
    }
    let manifests = BTreeSet::from([
        plugin_root.join("plugin.json"),
        plugin_root
            .join(CLAUDE_PLUGIN_MANIFEST_DIR)
            .join("plugin.json"),
        plugin_root
            .join(branding::PLUGIN_MANIFEST_DIR)
            .join("plugin.json"),
    ]);
    for manifest in manifests {
        if !manifest.is_file() {
            continue;
        }
        let raw = fs::read_to_string(&manifest)
            .map_err(|error| format!("failed to read {}: {error}", manifest.display()))?;
        let mut value = serde_json::from_str::<serde_json::Value>(&raw)
            .map_err(|error| format!("invalid {}: {error}", manifest.display()))?;
        if value
            .as_object_mut()
            .and_then(|object| object.remove("mcpServers"))
            .is_some()
        {
            let bytes = serde_json::to_vec(&value)
                .map_err(|error| format!("failed to serialize {}: {error}", manifest.display()))?;
            atomic_write(&manifest, &bytes)?;
        }
    }
    Ok(())
}

fn aggregate_mock_calls(
    paths: &[PathBuf],
) -> Result<(EvalMockCalls, Option<EvalMockAborted>), String> {
    let mut total = 0u64;
    let mut errors = 0u64;
    let mut unmocked = BTreeMap::<String, u64>::new();
    let mut aborted = None;
    for path in paths {
        let raw = match fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "failed to read mock call log {}: {error}",
                    path.display()
                ));
            }
        };
        for (index, line) in raw.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let record =
                serde_json::from_str::<crate::commands::plugin_eval_mock::MockCallRecord>(line)
                    .map_err(|error| {
                        format!(
                            "invalid mock call record in {} on line {}: {error}",
                            path.display(),
                            index + 1
                        )
                    })?;
            total = total.saturating_add(1);
            if record.error.is_some() {
                errors = errors.saturating_add(1);
            }
            if record.unmocked {
                *unmocked.entry(record.tool.clone()).or_default() += 1;
            }
            if aborted.is_none() {
                if let Some(reason) = record.abort_reason {
                    aborted = Some(EvalMockAborted {
                        server: record.server,
                        tool: record.tool,
                        reason: record.error.unwrap_or(reason),
                    });
                }
            }
        }
    }
    Ok((
        EvalMockCalls {
            total,
            errors,
            unmocked: unmocked
                .into_iter()
                .map(|(tool, count)| EvalMockUnmocked { tool, count })
                .collect(),
        },
        aborted,
    ))
}

fn discover_eval_mocks(
    cli: &Cli,
    plugin_root: &Path,
    case: &EvalCase,
) -> Result<Option<DiscoveredEvalMocks>, String> {
    let eval_dir = resolve_eval_dir(cli.eval_dir.as_deref(), plugin_root)?;
    let mut servers = BTreeMap::<String, MockServerAccumulator>::new();
    load_mock_tree(&plugin_root.join(eval_dir).join("mocks"), &mut servers)?;
    if let Some(case_dir) = case.source.parent() {
        load_mock_tree(&case_dir.join("mocks"), &mut servers)?;
    }
    if servers.is_empty() {
        return Ok(None);
    }

    let declared_servers = plugin_declared_mcp_servers(plugin_root);
    let plugin_name = plugin_runtime_name(plugin_root);
    let mut warnings = Vec::new();
    let mut runtime = Vec::new();
    let report_servers = servers
        .into_iter()
        .map(|(server, data)| {
            let shadow = declared_servers.contains(&server);
            let registered_name = if shadow {
                plugin_name
                    .as_deref()
                    .map(|name| format!("plugin:{name}:{server}"))
                    .unwrap_or_else(|| server.clone())
            } else {
                server.clone()
            };
            let missing_descriptions = data
                .tools
                .keys()
                .filter(|tool| !data.described_tools.contains_key(*tool))
                .cloned()
                .collect::<Vec<_>>();
            if !missing_descriptions.is_empty() {
                warnings.push(format!(
                    "{server}: no _tools.json entry for {} — served with a permissive schema and no description; save the server's tools/list response as mocks/{server}/_tools.json so the model sees the real tool",
                    missing_descriptions.join(", ")
                ));
            }
            let report_tools = data
                .tools
                .iter()
                .map(|(tool, responder)| EvalMockTool {
                    tool: tool.clone(),
                    responder: responder.kind.clone(),
                })
                .collect();
            let mut runtime_tools = BTreeMap::<String, crate::commands::plugin_eval_mock::MockToolSpec>::new();
            for (tool, description) in &data.described_tools {
                runtime_tools.insert(
                    tool.clone(),
                    mock_tool_spec_from_description(tool, description),
                );
            }
            for (tool, responder) in data.tools {
                let entry = runtime_tools.entry(tool.clone()).or_insert_with(|| {
                    crate::commands::plugin_eval_mock::MockToolSpec {
                        name: tool.clone(),
                        description: String::new(),
                        input_schema: serde_json::json!({
                            "type": "object",
                            "additionalProperties": true
                        }),
                        responder: None,
                    }
                });
                entry.responder = Some(crate::commands::plugin_eval_mock::FixedResponderSpec {
                    responder_type: responder.kind,
                    body: Some(responder.body),
                    base_dir: Some(responder.base_dir),
                    content: None,
                    error: responder.error,
                    expect: responder.expect,
                });
            }
            runtime.push(RuntimeMockServer {
                registered_name,
                spec: crate::commands::plugin_eval_mock::MockServerSpec {
                    server: server.clone(),
                    tools: runtime_tools.into_values().collect(),
                },
            });
            EvalMockServer {
                server,
                kind: if shadow { "shadow" } else { "standalone" }.to_string(),
                tools: report_tools,
            }
        })
        .collect();
    Ok(Some(DiscoveredEvalMocks {
        report: EvalMocks {
            servers: report_servers,
            warnings,
            calls: EvalMockCalls {
                total: 0,
                errors: 0,
                unmocked: Vec::new(),
            },
        },
        runtime,
    }))
}

fn mock_tool_spec_from_description(
    fallback_name: &str,
    value: &serde_json::Value,
) -> crate::commands::plugin_eval_mock::MockToolSpec {
    crate::commands::plugin_eval_mock::MockToolSpec {
        name: value
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(fallback_name)
            .to_string(),
        description: value
            .get("description")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        input_schema: value
            .get("inputSchema")
            .cloned()
            .filter(serde_json::Value::is_object)
            .unwrap_or_else(
                || serde_json::json!({ "type": "object", "additionalProperties": true }),
            ),
        responder: None,
    }
}

fn plugin_runtime_name(plugin_root: &Path) -> Option<String> {
    let candidates = [
        plugin_manifest_path(plugin_root),
        plugin_root
            .join(branding::PLUGIN_MANIFEST_DIR)
            .join("plugin.json"),
    ];
    candidates.into_iter().find_map(|manifest| {
        let raw = fs::read_to_string(manifest).ok()?;
        serde_json::from_str::<serde_json::Value>(&raw)
            .ok()?
            .get("name")?
            .as_str()
            .map(str::to_string)
    })
}

fn plugin_declared_mcp_servers(plugin_root: &Path) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let root_config = plugin_root.join(".mcp.json");
    if let Ok(raw) = fs::read_to_string(root_config) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) {
            let servers = value
                .get("mcpServers")
                .and_then(serde_json::Value::as_object)
                .or_else(|| value.as_object());
            if let Some(servers) = servers {
                names.extend(servers.keys().cloned());
            }
        }
    }
    for manifest in [
        plugin_manifest_path(plugin_root),
        plugin_root
            .join(branding::PLUGIN_MANIFEST_DIR)
            .join("plugin.json"),
    ] {
        if let Ok(raw) = fs::read_to_string(manifest) {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) {
                if let Some(servers) = value
                    .get("mcpServers")
                    .and_then(serde_json::Value::as_object)
                {
                    names.extend(servers.keys().cloned());
                }
            }
        }
    }
    names
}

fn load_mock_tree(
    root: &Path,
    servers: &mut BTreeMap<String, MockServerAccumulator>,
) -> Result<(), String> {
    if !root.is_dir() {
        return Ok(());
    }
    reject_symlink(root)?;
    let mut entries = fs::read_dir(root)
        .map_err(|error| format!("failed to read mocks directory {}: {error}", root.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            format!(
                "failed to inspect mocks directory {}: {error}",
                root.display()
            )
        })?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let ty = entry
            .file_type()
            .map_err(|error| format!("failed to inspect {}: {error}", entry.path().display()))?;
        if ty.is_symlink() || !ty.is_dir() {
            continue;
        }
        let server_name = entry.file_name().to_string_lossy().into_owned();
        if !is_safe_mock_segment(&server_name) {
            return Err(format!(
                "mock server directory name {server_name:?} must use only letters, digits, '_' and '-'"
            ));
        }
        let server_dir = entry.path();
        let server = servers.entry(server_name).or_default();
        load_mock_server(&server_dir, server)?;
    }
    Ok(())
}

fn load_mock_server(root: &Path, server: &mut MockServerAccumulator) -> Result<(), String> {
    let server_prompt = root.join("_server.md");
    if server_prompt.is_file() {
        return Err(format!(
            "{} uses an agent mock responder, which Claude Code 2.1.245 does not support yet",
            server_prompt.display()
        ));
    }

    let tools_path = root.join("_tools.json");
    if tools_path.is_file() {
        let raw = fs::read_to_string(&tools_path)
            .map_err(|error| format!("failed to read {}: {error}", tools_path.display()))?;
        let value = serde_json::from_str::<serde_json::Value>(&raw)
            .map_err(|error| format!("invalid {}: {error}", tools_path.display()))?;
        let tools = value
            .get("tools")
            .and_then(serde_json::Value::as_array)
            .or_else(|| value.as_array());
        if let Some(tools) = tools {
            for tool in tools {
                if let Some(name) = tool.get("name").and_then(serde_json::Value::as_str) {
                    if !is_safe_mock_segment(name) {
                        return Err(format!(
                            "mock tool name {name:?} in {} must use only letters, digits, '_' and '-'",
                            tools_path.display()
                        ));
                    }
                    server
                        .described_tools
                        .insert(name.to_string(), tool.clone());
                }
            }
        }
    }

    let mut entries = fs::read_dir(root)
        .map_err(|error| format!("failed to read mock server {}: {error}", root.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to inspect mock server {}: {error}", root.display()))?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let ty = entry
            .file_type()
            .map_err(|error| format!("failed to inspect {}: {error}", path.display()))?;
        if ty.is_symlink()
            || !ty.is_file()
            || path.extension().and_then(|ext| ext.to_str()) != Some("md")
        {
            continue;
        }
        let Some(tool) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if tool == "_server" {
            continue;
        }
        if !is_safe_mock_segment(tool) {
            return Err(format!(
                "mock tool filename {tool:?} must use only letters, digits, '_' and '-'"
            ));
        }
        let raw = fs::read_to_string(&path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        let (frontmatter, body) = parse_markdown_frontmatter(&raw, &path)?;
        let kind = yaml_string(&frontmatter, "type").unwrap_or_else(|| "fixed".to_string());
        if kind != "fixed" {
            return Err(format!(
                "{} uses unsupported mock responder type {kind:?}; Claude Code 2.1.245 accepts only fixed responders",
                path.display()
            ));
        }
        let expect = yaml_value(&frontmatter, "expect")
            .map(serde_json::to_value)
            .transpose()
            .map_err(|error| format!("invalid expect in {}: {error}", path.display()))?;
        server.tools.insert(
            tool.to_string(),
            MockResponderAccumulator {
                kind,
                body,
                base_dir: path.parent().unwrap_or(root).to_path_buf(),
                error: yaml_value(&frontmatter, "error")
                    .and_then(YamlValue::as_bool)
                    .unwrap_or(false),
                expect,
            },
        );
    }
    Ok(())
}

fn is_safe_mock_segment(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
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
        aborted: None,
        mocks: None,
    }
}

/// Build the single `--settings` argument the eval child receives, or `None`
/// when there is nothing to say.
///
/// One argument or none: a second `--settings` occurrence silently replaces
/// the first, so the harness's plugin-enablement keys and the arm's own
/// settings have to be merged here rather than passed separately. The arm
/// wins on a key collision, since an arm exists precisely to change what the
/// child runs under.
fn eval_child_settings(
    disabled_plugin_ids: &[String],
    arm_settings: Option<&serde_json::Value>,
) -> Option<String> {
    let mut settings = serde_json::Map::new();
    if !disabled_plugin_ids.is_empty() {
        let disabled = disabled_plugin_ids
            .iter()
            .map(|id| (id.clone(), serde_json::Value::Bool(false)))
            .collect::<serde_json::Map<_, _>>();
        settings.insert("enabledPlugins".to_string(), disabled.into());
    }
    if let Some(serde_json::Value::Object(arm)) = arm_settings {
        for (key, value) in arm {
            settings.insert(key.clone(), value.clone());
        }
    }
    (!settings.is_empty()).then(|| serde_json::Value::Object(settings).to_string())
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
    mock_runtime: Option<&PreparedMockRuntime>,
    arm_settings: Option<&serde_json::Value>,
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
    if let Some(runtime) = mock_runtime {
        command
            .arg("--mcp-config")
            .arg(&runtime.mcp_config)
            .arg("--strict-mcp-config")
            .arg("--disallowedTools")
            .arg("MCP");
    }
    if let Some(settings) = eval_child_settings(disabled_plugin_ids, arm_settings) {
        command.arg("--settings").arg(settings);
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
        GraderDefinition::Regex { pattern, .. } => {
            let matched = regex::Regex::new(pattern)
                .map(|regex| regex.is_match(output))
                .unwrap_or(false);
            (
                GraderResult::from_definition(
                    grader,
                    Some(if matched { 1.0 } else { 0.0 }),
                    matched,
                    if matched {
                        format!("output matched /{pattern}/")
                    } else {
                        format!("output did not match /{pattern}/")
                    },
                    false,
                ),
                0.0,
            )
        }
        GraderDefinition::ToolOrder { tools, .. } => {
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
                GraderResult::from_definition(
                    grader,
                    Some(if matched { 1.0 } else { 0.0 }),
                    matched,
                    if matched {
                        format!("tools appeared in order: {}", tools.join(", "))
                    } else {
                        format!(
                            "required tool order {} was not observed in {}",
                            tools.join(", "),
                            tools_used.join(", ")
                        )
                    },
                    false,
                ),
                0.0,
            )
        }
        GraderDefinition::ToolUsed { tool, .. } => {
            let matched = tools_used.iter().any(|actual| tool_matches(tool, actual));
            (
                GraderResult::from_definition(
                    grader,
                    Some(if matched { 1.0 } else { 0.0 }),
                    matched,
                    if matched {
                        format!("tool {tool:?} was used")
                    } else {
                        format!("tool {tool:?} was not used")
                    },
                    false,
                ),
                0.0,
            )
        }
        GraderDefinition::FileExists { path, .. } => {
            let exists = confined_output_path(cwd, path)
                .and_then(|path| fs::metadata(path).map_err(|error| error.to_string()))
                .map(|metadata| metadata.is_file())
                .unwrap_or(false);
            (
                GraderResult::from_definition(
                    grader,
                    Some(if exists { 1.0 } else { 0.0 }),
                    exists,
                    if exists {
                        format!("{} exists", path.display())
                    } else {
                        format!("{} does not exist", path.display())
                    },
                    false,
                ),
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
                    "Compare the candidate response to the baseline response for the same \
                     task. Score how well the candidate improves on or preserves the baseline \
                     under the rubric. Return only JSON with fields score \
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
                    GraderResult::from_definition(
                        grader,
                        Some(0.0),
                        false,
                        format!("failed to create judge directory: {error}"),
                        false,
                    ),
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
                None,
                // The judge grades under stock settings. An arm's settings
                // belong to the run being graded, never to the grader.
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
                GraderResult::from_definition(grader, Some(score), score >= 1.0, reason, false),
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
    let arms = parse_yaml_arms(map, &name)?;
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
        arms,
    })
}

/// Reserved arm labels. The ablation path already publishes runs under these,
/// so a case may not also mint them and collide in the same report.
const RESERVED_ARM_LABELS: [&str; 2] = ["with", "without"];

fn parse_yaml_arms(map: &Mapping, case_name: &str) -> Result<Vec<EvalArm>, String> {
    let Some(value) = yaml_value(map, "arms") else {
        return Ok(Vec::new());
    };
    let entries = value
        .as_sequence()
        .ok_or_else(|| format!("eval case {case_name:?}: arms must be a list"))?;
    if entries.len() < 2 {
        return Err(format!(
            "eval case {case_name:?}: arms needs at least two entries, or none at all -- \
             a single arm is the ordinary case and has nothing to compare against"
        ));
    }
    let mut arms = Vec::with_capacity(entries.len());
    let mut seen = Vec::new();
    for entry in entries {
        let entry = entry
            .as_mapping()
            .ok_or_else(|| format!("eval case {case_name:?}: each arm must be a YAML object"))?;
        let label = yaml_string(entry, "label")
            .ok_or_else(|| format!("eval case {case_name:?}: each arm needs a label"))?;
        let label = label.trim().to_string();
        if label.is_empty() {
            return Err(format!(
                "eval case {case_name:?}: an arm label cannot be blank"
            ));
        }
        if RESERVED_ARM_LABELS.contains(&label.as_str()) {
            return Err(format!(
                "eval case {case_name:?}: {label:?} is reserved for the ablation arms"
            ));
        }
        if seen.contains(&label) {
            return Err(format!(
                "eval case {case_name:?}: two arms are both labelled {label:?}"
            ));
        }
        seen.push(label.clone());
        let prompt = yaml_string(entry, "prompt");
        if prompt.as_deref().is_some_and(|text| text.trim().is_empty()) {
            return Err(format!(
                "eval case {case_name:?}: arm {label:?} declares an empty prompt; \
                 omit the key to inherit the case prompt"
            ));
        }
        let settings = match yaml_value(entry, "settings") {
            None => None,
            Some(raw) => {
                let json = serde_json::to_value(raw).map_err(|error| {
                    format!("eval case {case_name:?}: arm {label:?} settings are not JSON: {error}")
                })?;
                if !json.is_object() {
                    return Err(format!(
                        "eval case {case_name:?}: arm {label:?} settings must be an object"
                    ));
                }
                Some(json)
            }
        };
        arms.push(EvalArm {
            label,
            prompt,
            settings,
        });
    }
    Ok(arms)
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
        // A prose case is a prompt file; declaring arms needs `case.yaml`.
        arms: Vec::new(),
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
        return Ok((Mapping::new(), normalized.trim().to_string()));
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
    Ok((mapping, rest[end + 5..].trim().to_string()))
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
    let explicit_eval_dir = args.eval_dir.as_deref().or(parent_eval_dir);
    if explicit_eval_dir.is_none() && !is_plugin_or_skill_root(cwd) {
        eprintln!(
            "Error: {} is not a plugin or skill folder — run `claude plugin eval init` from the plugin's root folder, or pass --eval-dir to scaffold here on purpose.",
            cwd.display()
        );
        return RUNTIME_ERROR;
    }
    if explicit_eval_dir.is_none() {
        if let Some(warning) = manifest_eval_setting(cwd).warning {
            eprintln!("{warning}");
        }
    }
    // `evalDir: c.evalDir ?? a.opts().evalDir` — init's own flag, else the
    // parent `plugin eval --eval-dir`, else the manifest / `evals/`.
    let eval_dir = match resolve_eval_dir(explicit_eval_dir, cwd) {
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

fn is_plugin_or_skill_root(root: &Path) -> bool {
    plugin_manifest_path(root).is_file() || root.join("SKILL.md").is_file()
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
    Ok(manifest_eval_setting(plugin_root)
        .value
        .unwrap_or_else(|| DEFAULT_EVAL_DIR.to_string()))
}

#[derive(Default)]
struct ManifestEvalSetting {
    value: Option<String>,
    warning: Option<String>,
}

fn manifest_eval_setting(plugin_root: &Path) -> ManifestEvalSetting {
    let manifest = plugin_manifest_path(plugin_root);
    let Ok(text) = fs::read_to_string(&manifest) else {
        return ManifestEvalSetting::default();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return ManifestEvalSetting::default();
    };
    if let Some(raw) = value
        .get("experimental")
        .and_then(|experimental| experimental.get("evals"))
    {
        let Some(raw_name) = raw.as_str() else {
            return ManifestEvalSetting {
                value: None,
                warning: Some(format!(
                    "Warning: ignoring experimental.evals {} in {} — it must be a string naming a directory relative to the plugin root, set as \"experimental\": {{\"evals\": \"quality/evals\"}}; using evals/ (fix the manifest or pass --eval-dir)",
                    raw,
                    manifest.display()
                )),
            };
        };
        return match validate_eval_dir_name(raw_name) {
            Ok(name) => ManifestEvalSetting {
                value: Some(name),
                warning: None,
            },
            Err(_) => {
                let reason = if Path::new(raw_name).is_absolute() {
                    "it must be a relative path inside the plugin (e.g. quality/evals), not absolute"
                } else {
                    "it must stay inside the plugin and may not contain a parent-directory component"
                };
                ManifestEvalSetting {
                    value: None,
                    warning: Some(format!(
                        "Warning: ignoring experimental.evals {raw_name:?} in {} — {reason}; using evals/ (fix the manifest or pass --eval-dir)",
                        manifest.display()
                    )),
                }
            }
        };
    }
    if let Some(raw) = value.get("evals") {
        return ManifestEvalSetting {
            value: None,
            warning: Some(format!(
                "Warning: ignoring the top-level \"evals\" key in {} — set it as \"experimental\": {{\"evals\": {raw}}} (or pass --eval-dir); using evals/",
                manifest.display()
            )),
        };
    }
    ManifestEvalSetting::default()
}

/// Valid `experimental.evals` from the selected plugin manifest.
fn manifest_eval_dir(plugin_root: &Path) -> Option<String> {
    manifest_eval_setting(plugin_root).value
}

fn plugin_manifest_path(plugin_root: &Path) -> PathBuf {
    let packaged = plugin_root
        .join(CLAUDE_PLUGIN_MANIFEST_DIR)
        .join("plugin.json");
    if packaged.is_file() {
        packaged
    } else {
        plugin_root.join("plugin.json")
    }
}

fn plugin_manifest_identity(plugin_root: &Path) -> Option<(String, String)> {
    let text = fs::read_to_string(plugin_manifest_path(plugin_root)).ok()?;
    let value = serde_json::from_str::<serde_json::Value>(&text).ok()?;
    Some((
        value.get("name")?.as_str()?.to_string(),
        value
            .get("version")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
    ))
}

/// The eval dir is a relative directory below the plugin root. A trailing `/`
/// (the manifest's `evals/` spelling) is tolerated; absolute paths, `.` and
/// `..` components, backslashes, and file-like empty components are refused.
fn validate_eval_dir_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim().trim_end_matches('/');
    if trimmed.is_empty() || trimmed.contains('\\') {
        return Err(format!("invalid eval dir {name:?}"));
    }
    let mut segments = Vec::new();
    for component in Path::new(trimmed).components() {
        let Component::Normal(segment) = component else {
            return Err(format!("invalid eval dir {name:?}"));
        };
        let Some(segment) = segment.to_str() else {
            return Err(format!("invalid eval dir {name:?}"));
        };
        segments.push(segment);
    }
    if segments.is_empty() {
        return Err(format!("invalid eval dir {name:?}"));
    }
    Ok(segments.join("/"))
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
    let mut json = serialize_result_pretty(result)?;
    json.push('\n');
    let results_root = resolve_eval_dir(cli.eval_dir.as_deref(), &result.suite.root)?;
    let results_base = if result.suite.plugin_id.is_some() {
        cwd.to_path_buf()
    } else {
        result.suite.root.clone()
    };
    let output_dir = cli.output_dir.clone().unwrap_or_else(|| {
        results_base
            .join(&results_root)
            .join("results")
            .join(result.started_at.replace([':', '.'], "-"))
    });
    fs::create_dir_all(&output_dir)
        .map_err(|error| format!("failed to create {}: {error}", output_dir.display()))?;
    atomic_write(&output_dir.join("aggregate-result.json"), json.as_bytes())?;

    let report_path = cli
        .report
        .as_deref()
        .map(|path| resolve_output_path(cwd, path))
        .unwrap_or_else(|| output_dir.join("report.html"));
    atomic_write(&report_path, render_html_report(result).as_bytes())?;
    let displayed_report = fs::canonicalize(&report_path).unwrap_or(report_path);
    eprintln!("Report: {}", displayed_report.display());
    match cli.json.as_deref() {
        Some(JSON_STDOUT_SENTINEL) => print!("{json}"),
        Some(path) => {
            let output_path = resolve_output_path(cwd, Path::new(path));
            atomic_write(&output_path, json.as_bytes())?;
            println!("Wrote {}", output_path.display());
        }
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
        for run in case.arms.runs() {
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

fn parse_non_negative_f64(value: &str) -> Result<f64, String> {
    let parsed: f64 = value
        .parse()
        .map_err(|_| "--max-cost-usd must be a non-negative number".to_string())?;
    if parsed.is_finite() && parsed >= 0.0 {
        Ok(parsed)
    } else {
        Err("--max-cost-usd must be a non-negative number".to_string())
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
            "--mocks",
            "off",
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
        assert_eq!(eval.mocks, "off");
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

    #[test]
    fn mocks_default_matches_the_oracle() {
        let eval = parse_eval(["lingxi-cli", "plugin", "eval", "."]);
        assert_eq!(eval.mocks, "record");
    }

    /// `plugin eval` must not be gated behind an env var 2.1.270 no longer
    /// reads. `run` is not unit-drivable (it needs a plugins dir, a home and a
    /// cwd), so pin its source instead — and assert a POSITIVE landmark from the
    /// same function first, so a renamed or emptied file fails loudly rather
    /// than passing this as a vacuous absence.
    #[test]
    fn plugin_eval_is_not_gated_behind_a_graduated_env_var() {
        const SRC: &str = include_str!("plugin_eval.rs");
        let landmark = "--json output path must end in ".to_string() + ".json";
        assert!(
            SRC.contains(&landmark),
            "instrument check: run's argument validation should be in this file"
        );
        // Match the CODE construct, not the bare name: the note above this
        // function names the variable in prose, and a bare-name needle matches
        // that instead — which is how this assertion first went red.
        let gate = "::var(\"CLAUDE_CODE_WALNUT".to_string();
        assert!(
            !SRC.contains(&gate),
            "the early-access gate graduated upstream; re-adding it makes the \
             whole subcommand unreachable"
        );
    }

    #[tokio::test]
    async fn zero_cost_ceiling_returns_defined_partial_empty_result() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("plugin");
        fs::create_dir_all(root.join(CLAUDE_PLUGIN_MANIFEST_DIR)).unwrap();
        fs::write(
            root.join(CLAUDE_PLUGIN_MANIFEST_DIR).join("plugin.json"),
            br#"{"name":"demo","version":"0.0.1"}"#,
        )
        .unwrap();
        let case_dir = root.join(DEFAULT_EVAL_DIR).join("case");
        fs::create_dir_all(&case_dir).unwrap();
        fs::write(case_dir.join("prompt.md"), "hello\n").unwrap();
        let root_text = root.to_string_lossy().into_owned();
        let cli = parse_eval([
            "lingxi-cli",
            "plugin",
            "eval",
            "--ablation",
            "none",
            "--max-cost-usd",
            "0",
            &root_text,
        ]);
        let (result, exit) =
            run_evaluation(&cli, &temp.path().join("plugins"), temp.path(), temp.path())
                .await
                .unwrap();
        assert_eq!(exit, PARTIAL_EXIT);
        assert!(result.partial);
        assert_eq!(result.partial_reason.as_deref(), Some("cost_ceiling"));
        assert!(result.cases.is_empty());
        assert_eq!(result.aggregates.cases_total, 0);
        assert_eq!(result.aggregates.cases_passed, 0);
        assert_eq!(result.aggregates.overall_score, 0.0);
        assert_eq!(result.aggregates.overall_pass_rate, 0.0);
        assert_eq!(result.aggregates.mean_delta, None);
    }

    #[test]
    fn max_cost_accepts_zero_and_rejects_negative_or_non_finite_values() {
        assert_eq!(parse_non_negative_f64("0").unwrap(), 0.0);
        assert!(parse_non_negative_f64("-1").is_err());
        assert!(parse_non_negative_f64("NaN").is_err());
        assert!(parse_non_negative_f64("inf").is_err());
    }

    #[test]
    fn record_mocks_discover_server_tool_and_oracle_warning() {
        let temp = tempfile::tempdir().unwrap();
        let case_dir = temp.path().join("evals").join("smoke");
        fs::create_dir_all(&case_dir).unwrap();
        fs::write(case_dir.join("prompt.md"), "Say hello\n").unwrap();
        let mock_dir = temp.path().join("evals").join("mocks").join("demo-server");
        fs::create_dir_all(&mock_dir).unwrap();
        fs::write(mock_dir.join("echo.md"), "mocked\n").unwrap();
        let case = discover_cases(temp.path(), DEFAULT_EVAL_DIR)
            .unwrap()
            .pop()
            .unwrap();
        let cli = parse_eval(["lingxi-cli", "plugin", "eval", "."]);
        let mocks = discover_eval_mocks(&cli, temp.path(), &case)
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::to_value(&mocks.report).unwrap(),
            serde_json::json!({
                "servers": [{
                    "server": "demo-server",
                    "kind": "standalone",
                    "tools": [{"tool": "echo", "responder": "fixed"}]
                }],
                "warnings": [
                    "demo-server: no _tools.json entry for echo — served with a permissive schema and no description; save the server's tools/list response as mocks/demo-server/_tools.json so the model sees the real tool"
                ],
                "calls": {"total": 0, "errors": 0, "unmocked": []}
            })
        );
        assert_eq!(mocks.runtime.len(), 1);
        assert_eq!(mocks.runtime[0].registered_name, "demo-server");
        assert_eq!(mocks.runtime[0].spec.tools.len(), 1);
        assert_eq!(
            mocks.runtime[0].spec.tools[0]
                .responder
                .as_ref()
                .unwrap()
                .body
                .as_deref(),
            Some("mocked")
        );
    }

    #[test]
    fn declared_plugin_server_is_shadowed_with_full_tools_list_schema() {
        let temp = tempfile::tempdir().unwrap();
        let manifest = temp
            .path()
            .join(CLAUDE_PLUGIN_MANIFEST_DIR)
            .join("plugin.json");
        fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        fs::write(
            &manifest,
            br#"{"name":"demo","mcpServers":{"jira":{"command":"real-server"}}}"#,
        )
        .unwrap();
        fs::write(
            temp.path().join(".mcp.json"),
            br#"{"mcpServers":{"jira":{"command":"also-real"}}}"#,
        )
        .unwrap();
        let case_dir = temp.path().join("evals").join("smoke");
        fs::create_dir_all(&case_dir).unwrap();
        fs::write(case_dir.join("prompt.md"), "Use Jira\n").unwrap();
        let mock_dir = temp.path().join("evals").join("mocks").join("jira");
        fs::create_dir_all(&mock_dir).unwrap();
        fs::write(
            mock_dir.join("_tools.json"),
            br#"{"tools":[{"name":"echo","description":"Echo","inputSchema":{"type":"object","properties":{"summary":{"type":"string"}}}},{"name":"unserved","description":"Denied","inputSchema":{"type":"object"}}]}"#,
        )
        .unwrap();
        fs::write(
            mock_dir.join("echo.md"),
            "---\ntype: fixed\nerror: true\nexpect:\n  summary: hello\n---\n\n{{input.summary}}\n",
        )
        .unwrap();
        let case = discover_cases(temp.path(), DEFAULT_EVAL_DIR)
            .unwrap()
            .pop()
            .unwrap();
        let cli = parse_eval(["lingxi-cli", "plugin", "eval", "."]);
        let mocks = discover_eval_mocks(&cli, temp.path(), &case)
            .unwrap()
            .unwrap();
        assert_eq!(mocks.report.servers[0].kind, "shadow");
        assert!(mocks.report.warnings.is_empty());
        assert_eq!(mocks.runtime[0].registered_name, "plugin:demo:jira");
        assert_eq!(mocks.runtime[0].spec.tools.len(), 2);
        let echo = mocks.runtime[0]
            .spec
            .tools
            .iter()
            .find(|tool| tool.name == "echo")
            .unwrap();
        assert_eq!(echo.description, "Echo");
        assert!(echo.responder.as_ref().unwrap().error);
        assert!(mocks.runtime[0]
            .spec
            .tools
            .iter()
            .find(|tool| tool.name == "unserved")
            .unwrap()
            .responder
            .is_none());
    }

    #[test]
    fn staged_plugin_removes_both_mcp_declaration_forms() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("plugin");
        fs::create_dir_all(root.join(CLAUDE_PLUGIN_MANIFEST_DIR)).unwrap();
        fs::write(
            root.join(CLAUDE_PLUGIN_MANIFEST_DIR).join("plugin.json"),
            br#"{"name":"demo","mcpServers":{"real":{"command":"server"}},"description":"keep"}"#,
        )
        .unwrap();
        fs::write(
            root.join(".mcp.json"),
            br#"{"mcpServers":{"real":{"command":"server"}}}"#,
        )
        .unwrap();
        adapt_staged_plugin_manifest(&root).unwrap();
        strip_staged_plugin_mcp(&root).unwrap();
        assert!(!root.join(".mcp.json").exists());
        let manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(root.join(CLAUDE_PLUGIN_MANIFEST_DIR).join("plugin.json")).unwrap(),
        )
        .unwrap();
        assert!(manifest.get("mcpServers").is_none());
        assert_eq!(manifest["description"], "keep");
        let adapted: serde_json::Value = serde_json::from_slice(
            &fs::read(root.join(branding::PLUGIN_MANIFEST_DIR).join("plugin.json")).unwrap(),
        )
        .unwrap();
        assert!(adapted.get("mcpServers").is_none());
        assert_eq!(adapted["name"], "demo");
    }

    #[test]
    fn mock_call_logs_aggregate_errors_unmocked_and_abort() {
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("calls.jsonl");
        let records = [
            crate::commands::plugin_eval_mock::MockCallRecord {
                server: "jira".to_string(),
                tool: "echo".to_string(),
                arguments: serde_json::json!({"summary":"ok"}),
                error: None,
                abort_reason: None,
                unmocked: false,
            },
            crate::commands::plugin_eval_mock::MockCallRecord {
                server: "jira".to_string(),
                tool: "missing".to_string(),
                arguments: serde_json::json!({}),
                error: Some("unmocked".to_string()),
                abort_reason: None,
                unmocked: true,
            },
            crate::commands::plugin_eval_mock::MockCallRecord {
                server: "jira".to_string(),
                tool: "echo".to_string(),
                arguments: serde_json::json!({"summary":1}),
                error: Some("expect mismatch".to_string()),
                abort_reason: Some("expect_mismatch".to_string()),
                unmocked: false,
            },
        ];
        let mut raw = records
            .iter()
            .map(|record| serde_json::to_string(record).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        raw.push('\n');
        atomic_write(&log, raw.as_bytes()).unwrap();
        let (calls, aborted) = aggregate_mock_calls(&[log]).unwrap();
        assert_eq!(calls.total, 3);
        assert_eq!(calls.errors, 2);
        assert_eq!(calls.unmocked.len(), 1);
        assert_eq!(calls.unmocked[0].tool, "missing");
        assert_eq!(calls.unmocked[0].count, 1);
        let aborted = aborted.unwrap();
        assert_eq!(aborted.server, "jira");
        assert_eq!(aborted.tool, "echo");
        assert_eq!(aborted.reason, "expect mismatch");
    }

    #[test]
    fn empty_suite_diagnostic_matches_enabled_oracle_and_writes_no_result() {
        assert_eq!(
            no_eval_cases_message(Path::new("/tmp/plugin"), "evals", None),
            "No eval cases found under /tmp/plugin.\nCases are expected in a evals/ directory under /tmp/plugin (the default), each case a directory containing case.yaml or prompt.md.\nRun `claude plugin eval init` for a guided interview, or `claude plugin eval init --bare <name>` to scaffold a blank case."
        );
        assert_eq!(
            no_eval_cases_message(Path::new("/tmp/plugin"), "checks", Some("checks")),
            "No eval cases found under /tmp/plugin.\nCases are expected in a checks/ directory under /tmp/plugin (from --eval-dir), each case a directory containing case.yaml or prompt.md.\nRun `claude plugin eval init --eval-dir checks` for a guided interview, or `claude plugin eval init --bare <name> --eval-dir checks` to scaffold a blank case."
        );

        let temp = tempfile::tempdir().unwrap();
        let manifest = plugin_manifest_path(temp.path());
        fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        fs::write(&manifest, r#"{"experimental":{"evals":"checks"}}"#).unwrap();
        assert_eq!(
            no_eval_cases_message(temp.path(), "checks", None),
            format!(
                "No eval cases found under {}.\nCases are expected in a checks/ directory under {} (from {}; pass --eval-dir to override), each case a directory containing case.yaml or prompt.md.\nRun `claude plugin eval init` for a guided interview, or `claude plugin eval init --bare <name>` to scaffold a blank case.",
                temp.path().display(),
                temp.path().display(),
                manifest.display()
            )
        );
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

        let manifest_dir = root.join(CLAUDE_PLUGIN_MANIFEST_DIR);
        fs::create_dir_all(&manifest_dir).unwrap();
        fs::write(
            manifest_dir.join("plugin.json"),
            br#"{"name":"p","experimental":{"evals":"nested/suites"}}"#,
        )
        .unwrap();
        assert_eq!(resolve_eval_dir(None, root).unwrap(), "nested/suites");

        fs::write(
            manifest_dir.join("plugin.json"),
            br#"{"name":"p","experimental":{"evals":"nested//./suites"}}"#,
        )
        .unwrap();
        assert_eq!(resolve_eval_dir(None, root).unwrap(), "nested/suites");

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
    fn invalid_manifest_eval_dir_warns_and_falls_back_to_default() {
        let temp = tempfile::tempdir().unwrap();
        let manifest = temp
            .path()
            .join(CLAUDE_PLUGIN_MANIFEST_DIR)
            .join("plugin.json");
        fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        fs::write(
            &manifest,
            br#"{"name":"p","experimental":{"evals":"/tmp/escape"}}"#,
        )
        .unwrap();
        let setting = manifest_eval_setting(temp.path());
        assert_eq!(setting.value, None);
        assert!(setting
            .warning
            .as_deref()
            .unwrap()
            .contains("not absolute; using evals/"));
        assert_eq!(
            resolve_eval_dir(None, temp.path()).unwrap(),
            DEFAULT_EVAL_DIR
        );

        fs::write(&manifest, br#"{"name":"p","evals":"checks"}"#).unwrap();
        let setting = manifest_eval_setting(temp.path());
        assert_eq!(setting.value, None);
        assert!(setting
            .warning
            .as_deref()
            .unwrap()
            .contains("ignoring the top-level \"evals\" key"));
    }

    #[test]
    fn plugin_eval_manifest_prefers_packaged_then_accepts_root_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let root_manifest = root.join("plugin.json");
        fs::write(
            &root_manifest,
            br#"{"name":"root","experimental":{"evals":"root-evals"}}"#,
        )
        .unwrap();
        assert_eq!(plugin_manifest_path(root), root_manifest);
        assert_eq!(resolve_eval_dir(None, root).unwrap(), "root-evals");

        let packaged_manifest = root.join(CLAUDE_PLUGIN_MANIFEST_DIR).join("plugin.json");
        fs::create_dir_all(packaged_manifest.parent().unwrap()).unwrap();
        fs::write(
            &packaged_manifest,
            br#"{"name":"packaged","experimental":{"evals":"packaged-evals"}}"#,
        )
        .unwrap();
        assert_eq!(plugin_manifest_path(root), packaged_manifest);
        assert_eq!(resolve_eval_dir(None, root).unwrap(), "packaged-evals");
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
        assert_eq!(eval.json.as_deref(), Some(JSON_STDOUT_SENTINEL));
        assert_eq!(eval.target, None);
    }

    #[tokio::test]
    async fn explicit_dash_json_path_is_rejected_before_early_access_gate() {
        let eval = parse_eval(["lingxi-cli", "plugin", "eval", "--json", "-", "."]);
        assert_eq!(eval.json.as_deref(), Some("-"));
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(
            run(&eval, temp.path(), temp.path(), temp.path()).await,
            RUNTIME_ERROR
        );
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

    #[tokio::test]
    async fn init_bare_requires_plugin_or_skill_root_without_eval_dir_override() {
        let temp = tempfile::tempdir().unwrap();
        let args = InitArgs {
            bare: true,
            eval_dir: None,
            interactive: false,
            interview: false,
            name: Some("smoke".to_string()),
        };
        assert_eq!(run_init(&args, None, temp.path()).await, RUNTIME_ERROR);
        assert!(!temp.path().join(DEFAULT_EVAL_DIR).exists());

        let branded_manifest = temp
            .path()
            .join(branding::PLUGIN_MANIFEST_DIR)
            .join("plugin.json");
        fs::create_dir_all(branded_manifest.parent().unwrap()).unwrap();
        fs::write(
            branded_manifest,
            r#"{"name":"lingxi-only","version":"0.0.1"}"#,
        )
        .unwrap();
        assert_eq!(run_init(&args, None, temp.path()).await, RUNTIME_ERROR);
        assert!(!temp.path().join(DEFAULT_EVAL_DIR).exists());

        let manifest = temp
            .path()
            .join(CLAUDE_PLUGIN_MANIFEST_DIR)
            .join("plugin.json");
        fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        fs::write(manifest, r#"{"name":"smoke","version":"0.0.1"}"#).unwrap();
        assert_eq!(run_init(&args, None, temp.path()).await, SUCCESS);
        assert!(temp
            .path()
            .join(DEFAULT_EVAL_DIR)
            .join("smoke")
            .join("prompt.md")
            .is_file());
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
            partial_reason: None,
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
                mean_delta: None,
            },
        };
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["schemaVersion"], 1);
        assert_eq!(json["durationSeconds"], 0.0);
        assert_eq!(json["costUsd"], 0.0);
        assert!(json["suite"].get("judgeModel").is_none());
        assert!(json["suite"].get("tagFilters").is_none());
        assert_eq!(json["aggregates"]["casesTotal"], 0);
        assert_eq!(json["aggregates"]["overallPassRate"], 0.0);
        assert_eq!(json["cases"], serde_json::json!([]));

        let rendered = serialize_result_pretty(&result).unwrap();
        assert!(rendered.contains("\"durationSeconds\": 0,"));
        assert!(rendered.contains("\"threshold\": 1,"));
        assert!(!rendered.contains("\"judgeModel\""));
        assert!(!rendered.contains("\"tagFilters\""));

        let temp = tempfile::tempdir().unwrap();
        let mut cli = parse_eval(["lingxi-cli", "plugin", "eval", "--json", "empty.json", "."]);
        cli.output_dir = Some(temp.path().join("must-not-exist"));
        emit_empty_json(&cli, &result, temp.path()).unwrap();
        let bytes = fs::read(temp.path().join("empty.json")).unwrap();
        assert_eq!(bytes.last(), Some(&b'\n'));
        assert!(!temp.path().join("must-not-exist").exists());
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
            weight: 2.0,
            reason: String::new(),
            skipped: false,
            with_only: false,
            scored: true,
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
                weight: 1.0,
                reason: "<img src=x onerror=y>".to_string(),
                skipped: false,
                with_only: false,
                scored: true,
            }],
            score: 0.0,
            error: Some("<b>error</b>".to_string()),
            aborted: None,
            mocks: None,
        };
        let result = AggregateResult {
            schema_version: 1,
            claude_version: "0.12.0".to_string(),
            started_at: "2026-01-01T00:00:00.000Z".to_string(),
            duration_seconds: 0.0,
            cost_usd: 0.0,
            partial: false,
            partial_reason: None,
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
                source: "prose".to_string(),
                prompt_markdown: "<iframe>prompt</iframe>".to_string(),
                model: None,
                runs_per_case: 1,
                timeout_seconds: 600,
                max_turns: 10,
                tags: Vec::new(),
                graders: Vec::new(),
                arms: EvalArms {
                    by_label: vec![("with".to_string(), vec![malicious_run])],
                },
                aggregates: EvalCaseAggregates {
                    score: 1.0,
                    pass_rate: 1.0,
                    score_without: None,
                    pass_rate_without: None,
                    delta: None,
                    per_arm: Vec::new(),
                },
            }],
            aggregates: EvalAggregates {
                cases_total: 1,
                cases_passed: 1,
                overall_score: 1.0,
                overall_pass_rate: 1.0,
                mean_delta: None,
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

        let temp = tempfile::tempdir().unwrap();
        let output_dir = temp.path().join("results").join("run");
        let mut cli = parse_eval(["lingxi-cli", "plugin", "eval", "--json", "result.json", "."]);
        cli.output_dir = Some(output_dir.clone());
        emit_outputs(&cli, &result, temp.path()).unwrap();
        let requested = fs::read(temp.path().join("result.json")).unwrap();
        let aggregate = fs::read(output_dir.join("aggregate-result.json")).unwrap();
        assert_eq!(requested, aggregate);
        assert_eq!(aggregate.last(), Some(&b'\n'));
        assert!(output_dir.join("report.html").is_file());
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

    fn case_with_arms(body: &str) -> Result<EvalCase, String> {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("case.yaml");
        fs::write(&path, body).unwrap();
        parse_case_yaml(&path)
    }

    #[test]
    fn arms_are_parsed_in_order_and_inherit_the_case_prompt() {
        let case = case_with_arms(
            r#"
name: fusion-vs-single
prompt: "refactor the parser"
arms:
  - label: single
  - label: fusion
    prompt: "/fusion refactor the parser"
    settings:
      fusion:
        enabled: true
"#,
        )
        .unwrap();

        let labels: Vec<&str> = case.arms.iter().map(|arm| arm.label.as_str()).collect();
        assert_eq!(labels, ["single", "fusion"]);
        // An omitted prompt inherits, which is what keeps the pair comparable.
        assert_eq!(case.arms[0].prompt, None);
        assert_eq!(
            case.arms[1].prompt.as_deref(),
            Some("/fusion refactor the parser")
        );
        assert_eq!(case.arms[0].settings, None);
        assert_eq!(
            case.arms[1].settings,
            Some(serde_json::json!({"fusion": {"enabled": true}}))
        );
    }

    #[test]
    fn arm_declarations_that_cannot_produce_a_comparison_are_refused() {
        let single = case_with_arms("name: c\nprompt: p\narms:\n  - label: only\n").unwrap_err();
        assert!(single.contains("at least two entries"), "got: {single}");

        let dup =
            case_with_arms("name: c\nprompt: p\narms:\n  - label: a\n  - label: a\n").unwrap_err();
        assert!(dup.contains("both labelled"), "got: {dup}");

        let reserved = case_with_arms("name: c\nprompt: p\narms:\n  - label: with\n  - label: b\n")
            .unwrap_err();
        assert!(reserved.contains("reserved"), "got: {reserved}");

        let blank = case_with_arms("name: c\nprompt: p\narms:\n  - label: \"  \"\n  - label: b\n")
            .unwrap_err();
        assert!(blank.contains("cannot be blank"), "got: {blank}");

        let unlabelled =
            case_with_arms("name: c\nprompt: p\narms:\n  - prompt: x\n  - label: b\n").unwrap_err();
        assert!(unlabelled.contains("needs a label"), "got: {unlabelled}");

        let scalar_settings = case_with_arms(
            "name: c\nprompt: p\narms:\n  - label: a\n    settings: 7\n  - label: b\n",
        )
        .unwrap_err();
        assert!(
            scalar_settings.contains("must be an object"),
            "got: {scalar_settings}"
        );

        let empty_prompt = case_with_arms(
            "name: c\nprompt: p\narms:\n  - label: a\n    prompt: \"  \"\n  - label: b\n",
        )
        .unwrap_err();
        assert!(empty_prompt.contains("omit the key"), "got: {empty_prompt}");
    }

    fn bare_case(prompt: &str) -> EvalCase {
        EvalCase {
            name: "c".into(),
            prompt: prompt.into(),
            expected_outcome: None,
            tags: Vec::new(),
            runs: None,
            model: None,
            timeout_seconds: None,
            max_turns: None,
            allowed_tools: Vec::new(),
            append_system_prompt: None,
            scaffold_script: None,
            graders: Vec::new(),
            source: PathBuf::from("case.yaml"),
            arms: Vec::new(),
        }
    }

    /// Both shapes have to reduce to one ordered list, because everything
    /// downstream -- the baseline grader, the delta, the report keys -- reads
    /// only that list.
    #[test]
    fn the_arm_plan_puts_the_baseline_first_for_both_shapes() {
        let plain = bare_case("ask");
        assert_eq!(
            arm_plan(&plain, "none")
                .iter()
                .map(|arm| (arm.label.clone(), arm.plugin_enabled))
                .collect::<Vec<_>>(),
            [("with".to_string(), true)]
        );
        assert_eq!(
            arm_plan(&plain, "with-without")
                .iter()
                .map(|arm| (arm.label.clone(), arm.plugin_enabled))
                .collect::<Vec<_>>(),
            [("without".to_string(), false), ("with".to_string(), true)]
        );

        let mut declared = bare_case("ask");
        declared.arms = vec![
            EvalArm {
                label: "single".into(),
                prompt: None,
                settings: None,
            },
            EvalArm {
                label: "fusion".into(),
                prompt: Some("/fusion ask".into()),
                settings: Some(serde_json::json!({"fusion": {"enabled": true}})),
            },
        ];
        // Declared arms REPLACE the ablation pair rather than nesting inside
        // it, so a with-without invocation cannot silently double the runs.
        for ablation in ["none", "with-without"] {
            let plan = arm_plan(&declared, ablation);
            assert_eq!(plan.len(), 2, "ablation {ablation} must not add arms");
            assert_eq!(plan[0].label, "single");
            assert_eq!(plan[0].prompt, "ask", "an omitted prompt inherits");
            assert_eq!(plan[1].prompt, "/fusion ask");
            assert!(plan.iter().all(|arm| arm.plugin_enabled));
        }
    }

    /// An arm that turns a subsystem on has to reach the child, and it has to
    /// do so WITHOUT dropping the plugin-enablement keys the harness relies on
    /// to keep a globally installed copy of the target out of the baseline.
    /// The shipped corpus is a real input to this parser, so parse it here
    /// rather than discovering a malformed case only when a paid run starts.
    #[test]
    fn the_shipped_fusion_corpus_parses_and_is_shaped_for_a_paired_delta() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../evals/fusion")
            .canonicalize()
            .expect("the fusion eval corpus ships in-tree");
        let mut cases = Vec::new();
        for entry in fs::read_dir(&root).expect("corpus root is readable") {
            let dir = entry.expect("corpus entry").path();
            if !dir.is_dir() {
                continue;
            }
            cases.push(
                parse_case_yaml(&dir.join("case.yaml"))
                    .unwrap_or_else(|error| panic!("{} does not parse: {error}", dir.display())),
            );
        }
        assert_eq!(cases.len(), 24, "the corpus is 24 cases");

        for case in &cases {
            let labels: Vec<&str> = case.arms.iter().map(|arm| arm.label.as_str()).collect();
            assert_eq!(
                labels,
                ["single", "fusion"],
                "{}: the baseline arm must come first, since every delta is measured against it",
                case.name
            );
            // The fusion arm must actually ask for Fusion, and must carry the
            // whole task rather than the first line of it.
            let fusion_prompt = case.arms[1]
                .prompt
                .as_deref()
                .expect("the fusion arm overrides the prompt");
            assert!(
                fusion_prompt.starts_with("/fusion --models "),
                "{}: the fusion arm must invoke /fusion",
                case.name
            );
            let task = case.prompt.trim();
            let carried = fusion_prompt
                .lines()
                .skip(1)
                .map(str::trim_end)
                .collect::<Vec<_>>()
                .join("\n");
            assert_eq!(
                carried.trim(),
                task,
                "{}: the fusion arm must carry the whole task",
                case.name
            );
            assert_eq!(
                case.arms[1].settings,
                Some(serde_json::json!({"fusion": {"enabled": true}})),
                "{}: the fusion arm must turn Fusion on for itself",
                case.name
            );
            assert!(
                case.arms[0].prompt.is_none() && case.arms[0].settings.is_none(),
                "{}: the baseline arm must differ from the case in nothing at all",
                case.name
            );
            // Two graders: one scores the answer, one scores it against the
            // baseline arm. Without the second there is no paired delta.
            assert!(
                case.graders
                    .iter()
                    .any(|g| matches!(g, GraderDefinition::Baseline { .. })),
                "{}: a paired delta needs a baseline grader",
                case.name
            );
            assert!(
                case.graders
                    .iter()
                    .any(|g| matches!(g, GraderDefinition::Llm { .. })),
                "{}: a case needs a rubric of its own, not only a comparison",
                case.name
            );
        }

        let mut tags: Vec<&str> = cases
            .iter()
            .flat_map(|case| case.tags.iter().map(String::as_str))
            .collect();
        tags.sort_unstable();
        let mut counts = std::collections::BTreeMap::new();
        for tag in tags {
            *counts.entry(tag).or_insert(0) += 1;
        }
        assert_eq!(
            counts,
            std::collections::BTreeMap::from([
                ("patch", 6),
                ("plan", 6),
                ("research", 6),
                ("review", 6)
            ]),
            "the corpus is six cases in each of the four categories"
        );
    }

    #[test]
    fn arm_settings_and_plugin_disablement_travel_in_one_argument() {
        let disabled = vec!["target".to_string()];
        let arm = serde_json::json!({"fusion": {"enabled": true}});

        assert_eq!(eval_child_settings(&[], None), None);

        let harness_only: serde_json::Value =
            serde_json::from_str(&eval_child_settings(&disabled, None).unwrap()).unwrap();
        assert_eq!(harness_only["enabledPlugins"]["target"], false);
        assert!(harness_only.get("fusion").is_none());

        let arm_only: serde_json::Value =
            serde_json::from_str(&eval_child_settings(&[], Some(&arm)).unwrap()).unwrap();
        assert_eq!(arm_only["fusion"]["enabled"], true);
        assert!(arm_only.get("enabledPlugins").is_none());

        let merged: serde_json::Value =
            serde_json::from_str(&eval_child_settings(&disabled, Some(&arm)).unwrap()).unwrap();
        assert_eq!(merged["enabledPlugins"]["target"], false);
        assert_eq!(merged["fusion"]["enabled"], true);

        // On a collision the arm wins: that is the whole point of declaring it.
        let overriding = serde_json::json!({"enabledPlugins": {"target": true}});
        let clashed: serde_json::Value =
            serde_json::from_str(&eval_child_settings(&disabled, Some(&overriding)).unwrap())
                .unwrap();
        assert_eq!(clashed["enabledPlugins"]["target"], true);
    }

    /// The report keys are the arm labels, and their ORDER is load-bearing:
    /// the first entry is the baseline every delta is measured against.
    #[test]
    fn arms_serialize_as_a_label_keyed_map_that_round_trips_in_order() {
        let arms = EvalArms {
            by_label: vec![
                ("single".to_string(), Vec::new()),
                ("fusion".to_string(), Vec::new()),
            ],
        };
        let json = serde_json::to_string(&arms).unwrap();
        assert_eq!(json, r#"{"single":[],"fusion":[]}"#);
        let restored: EvalArms = serde_json::from_str(&json).unwrap();
        assert_eq!(
            restored
                .by_label
                .iter()
                .map(|(label, _)| label.as_str())
                .collect::<Vec<_>>(),
            ["single", "fusion"],
            "a sorted map would report the wrong baseline"
        );
        assert!(restored.labelled("fusion").is_some());
        assert!(restored.labelled("absent").is_none());
    }
}
