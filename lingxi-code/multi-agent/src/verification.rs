//! Verification phase (design doc §Verification, Phase 6).
//!
//! Verification runs commands against the **final** workspace (after the
//! finalizer applied the winning patch) and classifies the result. Two
//! outcomes are kept strictly distinct (design doc §Verification failure
//! handling):
//!
//! - **timeout** → [`VerificationOutcome::Inconclusive`] /
//!   [`MultiAgentError::VerificationTimedOut`]: the result is unknown, so the
//!   run must NOT claim `complete`.
//! - **non-zero exit** → [`VerificationOutcome::Failed`] /
//!   [`MultiAgentError::VerificationFailed`]: definite failure evidence; the
//!   fix loop may retry up to [`crate::config::LimitConfig::max_iterations`]
//!   times.
//!
//! ## `cargo fmt --check` is excluded
//!
//! The default suite is `cargo test -p <crate>` + `cargo clippy -p <crate>
//! --all-targets -- -D warnings`. It deliberately **never** includes
//! `cargo fmt --check`: this repository is intentionally not fmt-clean, so a
//! format check would non-zero-exit even for a perfectly correct candidate and
//! make `complete` permanently unreachable (design doc §Verification ⚠️).
//!
//! ## changed-files → affected-crate mapping
//!
//! [`affected_crates`] maps a set of changed paths to the set of workspace
//! members they fall under, by longest-matching-path-prefix against the
//! `members` list parsed from the workspace `Cargo.toml`. This is an
//! independently-tested unit: without it the suite would silently degrade to
//! `cargo test --workspace`, which is slow and costly on every dual-LLM run.

use crate::error::MultiAgentError;
use std::collections::BTreeSet;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

/// One verification command (program + args) to run in the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationCommand {
    /// The program, e.g. `cargo`.
    pub program: String,
    /// Its arguments, e.g. `["test", "-p", "multi-agent"]`.
    pub args: Vec<String>,
}

impl VerificationCommand {
    /// Construct a command.
    #[must_use]
    pub fn new(program: impl Into<String>, args: Vec<String>) -> Self {
        Self { program: program.into(), args }
    }

    /// Human-readable display (`program arg1 arg2 …`) used in logs / errors.
    #[must_use]
    pub fn display(&self) -> String {
        if self.args.is_empty() {
            self.program.clone()
        } else {
            format!("{} {}", self.program, self.args.join(" "))
        }
    }
}

/// Source of the verification commands, in design-doc precedence order
/// (§Verification: task brief > self-report > arbiter > repo default).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandSource {
    /// Explicit in the task brief.
    TaskBrief,
    /// Provided by a candidate self-report.
    SelfReport,
    /// Required by the arbiter.
    Arbiter,
    /// Repo default suite, narrowed by changed files.
    RepoDefault,
}

/// Resolve the verification suite by precedence. The first non-empty source
/// wins; the repo default is the narrowed-by-changed-files fallback.
///
/// `changed_paths` are workspace-relative paths from the final patch; they
/// narrow the repo-default suite to the affected crates only (avoiding a full
/// `cargo test --workspace` on every run). When no affected crate can be
/// resolved the default falls back to the whole workspace.
#[must_use]
pub fn resolve_suite(
    task_brief_commands: &[VerificationCommand],
    self_report_commands: &[VerificationCommand],
    arbiter_commands: &[VerificationCommand],
    changed_paths: &[PathBuf],
    workspace_members: &[String],
) -> (Vec<VerificationCommand>, CommandSource) {
    if !task_brief_commands.is_empty() {
        return (task_brief_commands.to_vec(), CommandSource::TaskBrief);
    }
    if !self_report_commands.is_empty() {
        return (self_report_commands.to_vec(), CommandSource::SelfReport);
    }
    if !arbiter_commands.is_empty() {
        return (arbiter_commands.to_vec(), CommandSource::Arbiter);
    }
    (
        default_suite(changed_paths, workspace_members),
        CommandSource::RepoDefault,
    )
}

/// Build the repo-default suite from the affected crates. Excludes
/// `cargo fmt --check` by construction (see module docs).
#[must_use]
pub fn default_suite(
    changed_paths: &[PathBuf],
    workspace_members: &[String],
) -> Vec<VerificationCommand> {
    let crates = affected_crates(changed_paths, workspace_members);
    if crates.is_empty() {
        // Cannot determine affected crates → whole workspace (design doc:
        // "如果修改跨 workspace 或无法确定 affected crate: cargo test --workspace").
        return vec![
            VerificationCommand::new("cargo", vec!["test".into(), "--workspace".into()]),
            VerificationCommand::new(
                "cargo",
                vec![
                    "clippy".into(),
                    "--workspace".into(),
                    "--all-targets".into(),
                    "--".into(),
                    "-D".into(),
                    "warnings".into(),
                ],
            ),
        ];
    }
    let mut cmds = Vec::new();
    for krate in &crates {
        cmds.push(VerificationCommand::new(
            "cargo",
            vec!["test".into(), "-p".into(), krate.clone()],
        ));
    }
    for krate in &crates {
        cmds.push(VerificationCommand::new(
            "cargo",
            vec![
                "clippy".into(),
                "-p".into(),
                krate.clone(),
                "--all-targets".into(),
                "--".into(),
                "-D".into(),
                "warnings".into(),
            ],
        ));
    }
    cmds
}

/// Map a set of changed paths to the workspace-member crate names they fall
/// under (design doc §Verification: parse workspace member path prefixes).
///
/// Each changed path is matched against the member directory list; the
/// **longest matching prefix** wins (so `tools/shell-mobile` is preferred over
/// `tools/shell` for a path under `tools/shell-mobile/`). Paths under no member
/// (e.g. top-level `docs/`) contribute no crate. The crate name returned is the
/// last path component of the member dir (e.g. member `tools/shell` →
/// `shell`), matching how `cargo -p <name>` is invoked in this repo.
///
/// Returns a sorted, de-duplicated set.
#[must_use]
pub fn affected_crates(changed_paths: &[PathBuf], workspace_members: &[String]) -> BTreeSet<String> {
    // Normalize members to component vectors once.
    let members: Vec<(Vec<String>, String)> = workspace_members
        .iter()
        .map(|m| {
            let comps = path_components(Path::new(m));
            let name = comps.last().cloned().unwrap_or_else(|| m.clone());
            (comps, name)
        })
        .collect();

    let mut out = BTreeSet::new();
    for changed in changed_paths {
        let changed_comps = path_components(changed);
        // Find the member whose component path is a prefix of the changed
        // path, choosing the longest such prefix.
        let mut best: Option<&(Vec<String>, String)> = None;
        for member in &members {
            if is_prefix(&member.0, &changed_comps) {
                match best {
                    Some(b) if b.0.len() >= member.0.len() => {}
                    _ => best = Some(member),
                }
            }
        }
        if let Some((_, name)) = best {
            out.insert(name.clone());
        }
    }
    out
}

/// Split a path into its normal components as owned strings, dropping `.`,
/// root, and prefix components. `..` is kept literally (it can never match a
/// member prefix, which is the safe behavior).
fn path_components(p: &Path) -> Vec<String> {
    p.components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            Component::ParentDir => Some("..".to_string()),
            _ => None,
        })
        .collect()
}

/// Whether `prefix` is a leading sub-sequence of `full`.
fn is_prefix(prefix: &[String], full: &[String]) -> bool {
    prefix.len() <= full.len() && prefix.iter().zip(full).all(|(a, b)| a == b)
}

/// Result of running the verification suite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerificationOutcome {
    /// Every command exited zero.
    Passed,
    /// A command exited non-zero — definite failure evidence.
    Failed {
        /// The command that failed (display form).
        command: String,
        /// Process exit code, if known.
        exit_code: Option<i32>,
    },
    /// A command timed out — result inconclusive; MUST NOT claim complete.
    Inconclusive {
        /// The command that timed out (display form).
        command: String,
    },
}

impl VerificationOutcome {
    /// Whether verification proved the workspace good.
    #[must_use]
    pub fn passed(&self) -> bool {
        matches!(self, VerificationOutcome::Passed)
    }
}

/// Runs a single verification command in a working directory, returning either
/// its exit status text or signalling a timeout. Abstracted so the run loop is
/// testable without spawning real `cargo` processes.
#[async_trait::async_trait]
pub trait CommandRunner: Send + Sync {
    /// Run `command` in `cwd` with the given wall-clock `timeout`.
    ///
    /// Returns:
    /// - `Ok(CommandResult { exit_code, output })` when the process completed
    ///   (zero or non-zero), or
    /// - `Err(CommandError::TimedOut)` when it exceeded `timeout`, or
    /// - `Err(CommandError::Spawn(_))` when it could not be launched.
    async fn run(
        &self,
        command: &VerificationCommand,
        cwd: &Path,
        timeout: Duration,
    ) -> Result<CommandResult, CommandError>;
}

/// A completed command's result.
#[derive(Debug, Clone)]
pub struct CommandResult {
    /// Exit code (`None` if terminated by signal).
    pub exit_code: Option<i32>,
    /// Combined stdout+stderr, appended to the verification log.
    pub output: String,
}

impl CommandResult {
    /// Whether the command succeeded (exit 0).
    #[must_use]
    pub fn success(&self) -> bool {
        self.exit_code == Some(0)
    }
}

/// Why a command could not produce a definite exit status.
#[derive(Debug, Clone, thiserror::Error)]
pub enum CommandError {
    /// The command exceeded its wall-clock timeout.
    #[error("command timed out")]
    TimedOut,
    /// The command could not be spawned.
    #[error("spawn failed: {0}")]
    Spawn(String),
}

/// Runs the verification suite in `workspace`, appending each command's output
/// to `log`. Stops at the first non-zero exit (failure evidence) or the first
/// timeout (inconclusive). A spawn error is treated as failure evidence (a
/// command that cannot run cannot prove correctness).
///
/// The returned [`VerificationOutcome`] is the host's source of truth — the run
/// never infers verification success from LLM free text.
pub async fn run_suite(
    runner: &dyn CommandRunner,
    commands: &[VerificationCommand],
    workspace: &Path,
    per_command_timeout: Duration,
    log: &mut String,
) -> VerificationOutcome {
    for command in commands {
        log.push_str(&format!("$ {}\n", command.display()));
        match runner.run(command, workspace, per_command_timeout).await {
            Ok(result) => {
                log.push_str(&result.output);
                if !result.output.ends_with('\n') {
                    log.push('\n');
                }
                if !result.success() {
                    log.push_str(&format!(
                        "[verification] FAILED: {} (exit {:?})\n",
                        command.display(),
                        result.exit_code
                    ));
                    return VerificationOutcome::Failed {
                        command: command.display(),
                        exit_code: result.exit_code,
                    };
                }
                log.push_str(&format!("[verification] ok: {}\n", command.display()));
            }
            Err(CommandError::TimedOut) => {
                log.push_str(&format!(
                    "[verification] TIMEOUT: {} (inconclusive)\n",
                    command.display()
                ));
                return VerificationOutcome::Inconclusive { command: command.display() };
            }
            Err(CommandError::Spawn(e)) => {
                log.push_str(&format!(
                    "[verification] SPAWN-ERROR: {} ({e})\n",
                    command.display()
                ));
                return VerificationOutcome::Failed {
                    command: command.display(),
                    exit_code: None,
                };
            }
        }
    }
    log.push_str("[verification] all commands passed\n");
    VerificationOutcome::Passed
}

/// Convert a non-passing [`VerificationOutcome`] into the typed error the run
/// surfaces. Timeout maps to [`MultiAgentError::VerificationTimedOut`]
/// (inconclusive); non-zero exit maps to [`MultiAgentError::VerificationFailed`]
/// (definite). `Passed` maps to `Ok(())`.
pub fn outcome_to_result(
    outcome: &VerificationOutcome,
    log_path: PathBuf,
) -> Result<(), MultiAgentError> {
    match outcome {
        VerificationOutcome::Passed => Ok(()),
        VerificationOutcome::Failed { command, exit_code } => {
            Err(MultiAgentError::VerificationFailed {
                command: command.clone(),
                exit_code: *exit_code,
                log_path,
            })
        }
        VerificationOutcome::Inconclusive { command } => {
            Err(MultiAgentError::VerificationTimedOut { command: command.clone(), log_path })
        }
    }
}

/// Drives the bounded verification + fix loop (design doc §Verification failure
/// handling): run verification; on **non-zero exit**, if iterations remain,
/// hand the failure to the `fixer` (operating on the final workspace / winner
/// worktree) and re-run; on **timeout**, stop immediately as inconclusive (a
/// fix cannot be proven against an unknown result).
///
/// `max_iterations` is [`crate::config::LimitConfig::max_iterations`] — the
/// fix+verify budget, distinct from `maxReviewRounds`. `max_iterations == 0`
/// means run verification once with no fix attempts.
pub async fn verify_with_fix_loop(
    runner: &dyn CommandRunner,
    fixer: &dyn VerificationFixer,
    commands: &[VerificationCommand],
    workspace: &Path,
    per_command_timeout: Duration,
    max_iterations: u32,
    log: &mut String,
) -> VerificationOutcome {
    let mut iteration: u32 = 0;
    loop {
        log.push_str(&format!("== verification attempt {} ==\n", iteration + 1));
        let outcome = run_suite(runner, commands, workspace, per_command_timeout, log).await;
        match outcome {
            VerificationOutcome::Passed => return VerificationOutcome::Passed,
            // Timeout: inconclusive, never retried (cannot prove a fix against
            // an unknown result).
            inconclusive @ VerificationOutcome::Inconclusive { .. } => return inconclusive,
            failed @ VerificationOutcome::Failed { .. } => {
                if iteration >= max_iterations {
                    log.push_str("[verification] fix budget exhausted; marking failed\n");
                    return failed;
                }
                log.push_str("[verification] handing failure to fixer\n");
                if let Err(e) = fixer
                    .fix(&FixContext {
                        attempt: iteration + 1,
                        workspace: workspace.to_path_buf(),
                        failure_log: log.clone(),
                    })
                    .await
                {
                    log.push_str(&format!("[verification] fixer failed: {e}; marking failed\n"));
                    return failed;
                }
                iteration += 1;
            }
        }
    }
}

/// Input handed to a [`VerificationFixer`] when verification fails.
#[derive(Debug, Clone)]
pub struct FixContext {
    /// 1-based fix attempt number.
    pub attempt: u32,
    /// The workspace to repair (final workspace or winner worktree).
    pub workspace: PathBuf,
    /// The verification log accumulated so far (failure evidence).
    pub failure_log: String,
}

/// Repairs a failing verification in place (design doc: "修复仍在最终主工作区
/// 或 winner worktree 中单点进行"). Single-writer: the fixer is the only thing
/// that mutates the workspace between verification attempts.
#[async_trait::async_trait]
pub trait VerificationFixer: Send + Sync {
    /// Attempt to fix the failure described by `ctx`. Returns `Err` if the fix
    /// itself could not be applied (the loop then stops, marking failed).
    async fn fix(&self, ctx: &FixContext) -> Result<(), MultiAgentError>;
}

/// A fixer that performs no repair — verification runs exactly once. Useful as
/// the default when `max_iterations == 0` or when no fixer is wired yet.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopFixer;

#[async_trait::async_trait]
impl VerificationFixer for NoopFixer {
    async fn fix(&self, _ctx: &FixContext) -> Result<(), MultiAgentError> {
        Err(MultiAgentError::FinalizerFailed {
            reason: "no verification fixer wired (TODO(multi-agent))".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use std::sync::Mutex;

    fn members() -> Vec<String> {
        vec![
            "engine".into(),
            "multi-agent".into(),
            "tools/shell".into(),
            "tools/shell-mobile".into(),
            "llm-client".into(),
            "apps/cli".into(),
        ]
    }

    #[test]
    fn affected_crates_maps_changed_files_to_members() {
        let changed = vec![
            PathBuf::from("multi-agent/src/finalizer.rs"),
            PathBuf::from("multi-agent/src/verification.rs"),
            PathBuf::from("engine/src/settings/schema.rs"),
        ];
        let got = affected_crates(&changed, &members());
        let expected: BTreeSet<String> =
            ["multi-agent".to_string(), "engine".to_string()].into_iter().collect();
        assert_eq!(got, expected);
    }

    #[test]
    fn affected_crates_prefers_longest_prefix() {
        // A file under tools/shell-mobile must map to shell-mobile, not shell.
        let changed = vec![PathBuf::from("tools/shell-mobile/src/lib.rs")];
        let got = affected_crates(&changed, &members());
        assert_eq!(got, ["shell-mobile".to_string()].into_iter().collect());

        let changed = vec![PathBuf::from("tools/shell/src/lib.rs")];
        let got = affected_crates(&changed, &members());
        assert_eq!(got, ["shell".to_string()].into_iter().collect());
    }

    #[test]
    fn affected_crates_ignores_non_member_paths() {
        let changed = vec![
            PathBuf::from("docs/multi-agent-dual-llm-design.md"),
            PathBuf::from("README.md"),
        ];
        assert!(affected_crates(&changed, &members()).is_empty());
    }

    #[test]
    fn default_suite_has_no_cargo_fmt_and_targets_affected_crates() {
        let changed = vec![PathBuf::from("multi-agent/src/finalizer.rs")];
        let suite = default_suite(&changed, &members());
        // Targets the affected crate.
        assert!(suite.iter().any(|c| c.program == "cargo"
            && c.args == vec!["test".to_string(), "-p".to_string(), "multi-agent".to_string()]));
        // NO cargo fmt --check anywhere in the suite.
        for c in &suite {
            let joined = c.display();
            assert!(
                !joined.contains("fmt"),
                "default suite must never contain cargo fmt: {joined}"
            );
        }
    }

    #[test]
    fn default_suite_falls_back_to_workspace_when_unmapped() {
        let changed = vec![PathBuf::from("docs/x.md")];
        let suite = default_suite(&changed, &members());
        assert!(suite
            .iter()
            .any(|c| c.args == vec!["test".to_string(), "--workspace".to_string()]));
        for c in &suite {
            assert!(!c.display().contains("fmt"));
        }
    }

    #[test]
    fn resolve_suite_precedence_task_brief_wins() {
        let brief = vec![VerificationCommand::new("make", vec!["check".into()])];
        let (suite, src) = resolve_suite(&brief, &[], &[], &[], &members());
        assert_eq!(src, CommandSource::TaskBrief);
        assert_eq!(suite, brief);
    }

    #[test]
    fn resolve_suite_falls_through_to_repo_default() {
        let changed = vec![PathBuf::from("engine/src/lib.rs")];
        let (suite, src) = resolve_suite(&[], &[], &[], &changed, &members());
        assert_eq!(src, CommandSource::RepoDefault);
        assert!(suite
            .iter()
            .any(|c| c.args == vec!["test".to_string(), "-p".to_string(), "engine".to_string()]));
    }

    // --- run loop ---

    enum Step {
        Ok,
        Fail(i32),
        Timeout,
    }

    struct ScriptedRunner {
        steps: Mutex<Vec<Step>>,
        calls: AtomicUsize,
    }
    impl ScriptedRunner {
        fn new(steps: Vec<Step>) -> Self {
            Self { steps: Mutex::new(steps), calls: AtomicUsize::new(0) }
        }
    }
    #[async_trait::async_trait]
    impl CommandRunner for ScriptedRunner {
        async fn run(
            &self,
            _command: &VerificationCommand,
            _cwd: &Path,
            _timeout: Duration,
        ) -> Result<CommandResult, CommandError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let step = {
                let mut g = self.steps.lock().unwrap();
                if g.is_empty() {
                    Step::Ok
                } else {
                    g.remove(0)
                }
            };
            match step {
                Step::Ok => Ok(CommandResult { exit_code: Some(0), output: "ok\n".into() }),
                Step::Fail(code) => {
                    Ok(CommandResult { exit_code: Some(code), output: "boom\n".into() })
                }
                Step::Timeout => Err(CommandError::TimedOut),
            }
        }
    }

    #[tokio::test]
    async fn run_suite_passes_when_all_zero() {
        let runner = ScriptedRunner::new(vec![Step::Ok, Step::Ok]);
        let cmds = vec![
            VerificationCommand::new("cargo", vec!["test".into()]),
            VerificationCommand::new("cargo", vec!["clippy".into()]),
        ];
        let mut log = String::new();
        let out = run_suite(&runner, &cmds, Path::new("/ws"), Duration::from_secs(1), &mut log).await;
        assert_eq!(out, VerificationOutcome::Passed);
        assert!(log.contains("all commands passed"));
    }

    #[tokio::test]
    async fn run_suite_failure_is_definite_evidence() {
        let runner = ScriptedRunner::new(vec![Step::Fail(101)]);
        let cmds = vec![VerificationCommand::new("cargo", vec!["test".into()])];
        let mut log = String::new();
        let out = run_suite(&runner, &cmds, Path::new("/ws"), Duration::from_secs(1), &mut log).await;
        assert!(matches!(out, VerificationOutcome::Failed { exit_code: Some(101), .. }));
        let err = outcome_to_result(&out, PathBuf::from("/log")).unwrap_err();
        assert!(matches!(err, MultiAgentError::VerificationFailed { .. }));
    }

    #[tokio::test]
    async fn run_suite_timeout_is_inconclusive_not_failed() {
        let runner = ScriptedRunner::new(vec![Step::Timeout]);
        let cmds = vec![VerificationCommand::new("cargo", vec!["test".into()])];
        let mut log = String::new();
        let out = run_suite(&runner, &cmds, Path::new("/ws"), Duration::from_secs(1), &mut log).await;
        assert!(matches!(out, VerificationOutcome::Inconclusive { .. }));
        // Inconclusive maps to VerificationTimedOut — NOT VerificationFailed.
        let err = outcome_to_result(&out, PathBuf::from("/log")).unwrap_err();
        assert!(matches!(err, MultiAgentError::VerificationTimedOut { .. }));
    }

    /// A fixer that succeeds a fixed number of times, then refuses.
    struct CountingFixer {
        fixes_remaining: Mutex<u32>,
        calls: AtomicUsize,
    }
    impl CountingFixer {
        fn new(fixes: u32) -> Self {
            Self { fixes_remaining: Mutex::new(fixes), calls: AtomicUsize::new(0) }
        }
    }
    #[async_trait::async_trait]
    impl VerificationFixer for CountingFixer {
        async fn fix(&self, _ctx: &FixContext) -> Result<(), MultiAgentError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let mut g = self.fixes_remaining.lock().unwrap();
            if *g == 0 {
                return Err(MultiAgentError::FinalizerFailed { reason: "out of fixes".into() });
            }
            *g -= 1;
            Ok(())
        }
    }

    #[tokio::test]
    async fn fix_loop_retries_until_pass_within_budget() {
        // fail, then (after fix) pass.
        let runner = ScriptedRunner::new(vec![Step::Fail(1), Step::Ok]);
        let fixer = CountingFixer::new(1);
        let cmds = vec![VerificationCommand::new("cargo", vec!["test".into()])];
        let mut log = String::new();
        let out = verify_with_fix_loop(
            &runner,
            &fixer,
            &cmds,
            Path::new("/ws"),
            Duration::from_secs(1),
            2,
            &mut log,
        )
        .await;
        assert_eq!(out, VerificationOutcome::Passed);
        assert_eq!(fixer.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn fix_loop_exhausts_budget_then_fails() {
        // Always fails; max_iterations = 1 → verify, fix, verify, give up.
        let runner = ScriptedRunner::new(vec![Step::Fail(1), Step::Fail(1), Step::Fail(1)]);
        let fixer = CountingFixer::new(5);
        let cmds = vec![VerificationCommand::new("cargo", vec!["test".into()])];
        let mut log = String::new();
        let out = verify_with_fix_loop(
            &runner,
            &fixer,
            &cmds,
            Path::new("/ws"),
            Duration::from_secs(1),
            1,
            &mut log,
        )
        .await;
        assert!(matches!(out, VerificationOutcome::Failed { .. }));
        // Exactly one fix attempt before exhausting the budget.
        assert_eq!(fixer.calls.load(Ordering::SeqCst), 1);
        assert!(log.contains("fix budget exhausted"));
    }

    #[tokio::test]
    async fn fix_loop_timeout_stops_immediately_inconclusive() {
        let runner = ScriptedRunner::new(vec![Step::Timeout]);
        let fixer = CountingFixer::new(5);
        let cmds = vec![VerificationCommand::new("cargo", vec!["test".into()])];
        let mut log = String::new();
        let out = verify_with_fix_loop(
            &runner,
            &fixer,
            &cmds,
            Path::new("/ws"),
            Duration::from_secs(1),
            5,
            &mut log,
        )
        .await;
        assert!(matches!(out, VerificationOutcome::Inconclusive { .. }));
        // No fix attempt on a timeout (cannot prove a fix against unknown).
        assert_eq!(fixer.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn fix_loop_zero_iterations_runs_once() {
        let runner = ScriptedRunner::new(vec![Step::Fail(1)]);
        let fixer = CountingFixer::new(5);
        let cmds = vec![VerificationCommand::new("cargo", vec!["test".into()])];
        let mut log = String::new();
        let out = verify_with_fix_loop(
            &runner,
            &fixer,
            &cmds,
            Path::new("/ws"),
            Duration::from_secs(1),
            0,
            &mut log,
        )
        .await;
        assert!(matches!(out, VerificationOutcome::Failed { .. }));
        assert_eq!(fixer.calls.load(Ordering::SeqCst), 0, "no fix attempts when budget is 0");
    }
}
