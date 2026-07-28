//! `lingxi-cli auto-mode-setup` — the WIZARD-06 permission-hardening wizard's
//! CLI arg-grammar layer (byte-parity with claude-code 2.1.220
//! `auto-mode-setup`).
//!
//! This module is the FLAG-GRAMMAR front door for the non-interactive apply
//! path `auto-mode-setup [--request-id <id>] [--apply-target user|project]
//! --expect-sha256 <64-hex> --apply-file <path>`. It parses + order-validates
//! those flags into an [`ApplyFileInvocation`] and hands off to the already-built
//! permission pipeline (`permission::auto_mode_setup::{read_proposal_file,
//! evaluate_apply_file, validate_auto_mode_save}` + `permission::persist::
//! persist_auto_mode_save`), which performs the security-relevant work: the
//! path/read gate, the sha256 hash-verify (a MISSING `--expect-sha256` is
//! rejected THERE, so a lenient grammar can never skip the hash bind), the parse
//! + scope check, and the atomic settings write.
//!
//! Every grammar message + telemetry code below is byte-verified against the
//! 2.1.220 binary. Ordering precedence uses a left-to-right, first-violation-
//! wins walk over the canonical flag order (`--request-id` < `--apply-target` <
//! `--expect-sha256` < `--apply-file`); each specific message is emitted ONLY in
//! the structural situation its text describes, and flag arrangements the
//! binary's strings don't unambiguously attribute fall back to a `usage` error
//! carrying the oracle's generic [`PARSE_FALLBACK`] message rather than a
//! guessed one.
//!
//! The non-interactive propose entry (`--wizard posture=… scope=… depth=…
//! --propose`) is a whole-string grammar of its own and is matched first; its
//! answer values are validated against the sets the wizard actually offers,
//! because those answers authorise how far the recon reaches.

use std::path::{Path, PathBuf};

use clap::Args;
use permission::auto_mode_setup as pipeline;
use permission::{
    persist_auto_mode_save, PermissionPaths, PermissionUpdateDestination, PersistError,
};
use serde_json::Value;

use crate::exit_codes::{RUNTIME_ERROR, SUCCESS};

// ── grammar + apply pipeline (moved to `permission::auto_mode_argv`) ────────
//
// The grammar now lives below the CLI so the `/auto-mode-setup` slash command
// shares it byte-for-byte. Re-exported here so this module's public surface
// and its tests are unchanged.
pub use permission::auto_mode_argv::*;

// ── CLI dispatch (`lingxi-cli auto-mode-setup`) ──────────────────────────────

/// `auto-mode-setup` payload. The flags are parsed by the hand-rolled grammar
/// ([`parse_apply_file_args`]), so clap only captures the raw token stream
/// (`trailing_var_arg` + `allow_hyphen_values`) — matching the oracle, which
/// order-validates the flags itself.
#[derive(Debug, Clone, Args)]
pub struct Cli {
    /// Raw `auto-mode-setup` flags (e.g. `--expect-sha256 <hex> --apply-file <path>`).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

/// How a parsed outcome maps to process behavior: an exit code, an optional
/// `auto_mode_setup_write` telemetry code, and optional stderr text. Pure +
/// testable; [`run`] performs the actual emit/print/exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disposition {
    /// The process exit code.
    pub exit_code: i32,
    /// The `auto_mode_setup_write` telemetry code to emit, if any.
    pub telemetry_code: Option<String>,
    /// The stderr message to print, if any.
    pub stderr: Option<String>,
}

/// Map a grammar rejection to its process disposition (verified codes/messages).
fn dispose_grammar(e: &GrammarError) -> Disposition {
    Disposition {
        exit_code: RUNTIME_ERROR,
        telemetry_code: Some(e.code.to_string()),
        stderr: (!e.message.is_empty()).then(|| e.message.to_string()),
    }
}

/// Map an [`ApplyResult`] to its process disposition. Gate rejections carry the
/// pipeline's byte-exact code; a validation failure folds into `usage` (the
/// write region has no distinct validation code); success emits nothing (no
/// success code exists) and exits 0.
fn dispose_apply(result: &ApplyResult) -> Disposition {
    match result {
        ApplyResult::Wrote { .. } | ApplyResult::NoChange => Disposition {
            exit_code: SUCCESS,
            telemetry_code: None,
            stderr: None,
        },
        ApplyResult::Rejected { code, reason } => Disposition {
            exit_code: RUNTIME_ERROR,
            telemetry_code: Some(code.clone()),
            stderr: Some(reason.clone()),
        },
        ApplyResult::InvalidSave { reason } => Disposition {
            exit_code: RUNTIME_ERROR,
            telemetry_code: Some(CODE_USAGE.to_string()),
            stderr: Some(reason.clone()),
        },
    }
}

/// Apply a [`Disposition`]: emit telemetry, print stderr, return the exit code.
fn apply_disposition(d: Disposition) -> i32 {
    if let Some(code) = &d.telemetry_code {
        telemetry::emit_auto_mode_setup_write(code);
    }
    if let Some(msg) = &d.stderr {
        eprintln!("{msg}");
    }
    d.exit_code
}

/// Run `lingxi-cli auto-mode-setup`. Parses the raw flags, then either prints
/// usage (`--help`) or runs the `--apply-file` flow against the live settings.
///
/// A standalone apply has no loaded session policy, so the `Read`-deny overlay
/// is inactive (`is_read_denied` = `false`); the temp/config containment root
/// gate + the `O_NOFOLLOW`/`nlink==1` secure read still constrain what is read.
pub async fn run(cli: &Cli) -> i32 {
    match parse_apply_file_args(&cli.args) {
        Ok(AutoModeSetupInvocation::Help) => {
            print_usage();
            SUCCESS
        }
        Ok(AutoModeSetupInvocation::ApplyFile(inv)) => run_apply_file(&inv).await,
        Ok(AutoModeSetupInvocation::Propose(inv)) => run_propose(&inv).await,
        Err(e) => apply_disposition(dispose_grammar(&e)),
    }
}

/// Run `--wizard … --propose`: gather the recon, ask the model, print the
/// result JSON.
///
/// The proposal is PRINTED, never applied. That separation is the whole design
/// of this command: `--apply-file` exists so a human sees the proposal before
/// any of it reaches a settings file, and the oracle refuses a one-shot
/// `--apply` for exactly that reason.
async fn run_propose(inv: &ProposeInvocation) -> i32 {
    use permission::auto_mode_propose::{ProposeAnswers, ProposeOutcome};

    let answers = ProposeAnswers {
        posture: inv.posture.clone(),
        scope: inv.scope.clone(),
        depth: inv.depth.clone(),
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    let outcome = match propose_outcome(&answers, &cwd).await {
        Ok(outcome) => outcome,
        // A stack that will not resolve is an environment failure, not a failed
        // scan: report it as `api_failed` rather than inventing a recon result.
        Err(reason) => ProposeOutcome::Failed {
            code: permission::auto_mode_propose::PROPOSE_CODE_API_FAILED,
            reason,
            emit_telemetry: true,
        },
    };

    // The oracle records the run's code on both the failure and the qualified
    // success paths, and stays silent on `aborted`.
    match &outcome {
        ProposeOutcome::Ok(success) => {
            if let Some(code) = success.telemetry_code {
                telemetry::emit_auto_mode_setup_propose(code);
            }
        }
        ProposeOutcome::Failed {
            code,
            emit_telemetry,
            ..
        } => {
            if *emit_telemetry {
                telemetry::emit_auto_mode_setup_propose(code);
            }
        }
    }

    println!(
        "{}",
        propose_result_json(propose_result_body(&outcome), inv.request_id.as_deref())
    );
    match outcome {
        ProposeOutcome::Ok(_) => SUCCESS,
        ProposeOutcome::Failed { .. } => RUNTIME_ERROR,
    }
}

/// Resolve the LLM stack, then drive the propose orchestration over it.
///
/// `Err` means the stack itself could not be resolved (no credentials, an
/// unroutable default model); the run never reached the model.
async fn propose_outcome(
    answers: &permission::auto_mode_propose::ProposeAnswers,
    cwd: &Path,
) -> Result<permission::auto_mode_propose::ProposeOutcome, String> {
    use crate::commands::auto_mode_propose::{ApiProposeQuery, FsProposeGather};

    // No argv overrides: `auto-mode-setup` takes no `--model`, so the model and
    // the provider set come from settings + env exactly as a session's would.
    let cfg = crate::init::resolve_desktop_config(
        &crate::argv::Argv::default(),
        permission::PermissionMode::Default,
    );
    let stack = engine_desktop::resolve_llm_stack(&cfg)
        .await
        .map_err(|e| format!("The model call didn\u{2019}t start: {e}"))?;

    let model = stack.default_model_id.clone();
    let profile = stack.default_model_profile.clone();
    // The oracle derives the thinking flag from the MODEL (`IQt(r)`), not from
    // session config, and grants the no-thinking budget top-up when the model
    // has no thinking config. Mirror that off the resolved listing.
    let thinking = stack
        .default_listings
        .iter()
        .find(|l| l.request_model == model || l.display_model == model)
        .is_some_and(|l| l.supports_reasoning);
    // `subscription_signal` reads the plan from the live snapshot; an
    // unauthenticated or still-fetching session yields `None`, which renders as
    // the "unknown" signal rather than a guessed plan.
    let plan = stack
        .subscription
        .read()
        .ok()
        .and_then(|g| g.as_ref().and_then(|s| s.subscription_type.clone()));

    let gather = FsProposeGather {
        root: cwd.to_path_buf(),
        user_config_dir: crate::run::lingxi_home_dir(),
        // `getProjectDir(cwd)` — where this project's session transcripts live.
        transcript_dir: crate::run::lingxi_home_dir().join("projects").join(
            session::jsonl::path::project_dir_name(&cwd.to_string_lossy()),
        ),
        // `autoMode.classifyAllShell` has no settings key in this build, so the
        // recon reports the conservative (off) state rather than claiming a
        // setting it never read.
        classify_all_shell: false,
    };

    let service = std::sync::Arc::new(engine_desktop::api_service_from_stack(&cfg, cwd, stack));
    let query = ApiProposeQuery::new(service, model, profile, thinking);

    Ok(crate::commands::auto_mode_propose::run_propose_blocking(
        answers.clone(),
        plan,
        permission::auto_mode_defaults::DEFAULT_ENVIRONMENT
            .iter()
            .map(|s| (*s).to_string())
            .collect(),
        gather,
        query,
    )
    .await)
}

async fn run_apply_file(inv: &ApplyFileInvocation) -> i32 {
    let lingxi_home = crate::run::lingxi_home_dir();
    // `uNd` seeds its root set with BOTH `path.resolve(n)` and `fs.realpath(n)`
    // for each root, so either spelling of a root matches. On macOS this is not
    // optional: `temp_dir()` yields `/var/folders/…` while `/var` is a symlink
    // to `/private/var`, so a reviewing host that realpath'd the proposal path
    // would otherwise be refused as `bad_path` for a file that is literally in
    // the system temp directory.
    let mut roots = vec![std::env::temp_dir(), lingxi_home.clone()];
    for root in roots.clone() {
        if let Ok(canonical) = std::fs::canonicalize(&root) {
            if !roots.contains(&canonical) {
                roots.push(canonical);
            }
        }
    }
    let paths = PermissionPaths {
        lingxi_home,
        cwd: std::env::current_dir().unwrap_or_default(),
    };
    match execute_apply_file(inv, &roots, |_| false, &paths).await {
        Ok(result) => apply_disposition(dispose_apply(&result)),
        Err(err) => {
            telemetry::emit_auto_mode_setup_write("write_failed");
            eprintln!("Could not write the auto-mode settings: {err}");
            RUNTIME_ERROR
        }
    }
}

/// Print a short synopsis for `auto-mode-setup --help`.
fn print_usage() {
    eprintln!("Apply a reviewed auto-mode proposal to your settings.");
    eprintln!();
    eprintln!(
        "Usage: lingxi-cli auto-mode-setup [--request-id <id>] [--apply-target user|project] \\"
    );
    eprintln!("           --expect-sha256 <64-hex> --apply-file <path>");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| (*s).to_string()).collect()
    }
    const HEX: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const UUID: &str = "3f2504e0-4f89-11d3-9a0c-0305e82c3301";

    #[test]
    fn messages_are_byte_exact() {
        // Spot-check the unicode-bearing messages (em-dash U+2014, curly ’ U+2019).
        assert_eq!(
            APPLY_TARGET_BAD_VALUE,
            "--apply-target must be \"user\" or \"project\"."
        );
        assert!(APPLY_TARGET_TWICE.contains('\u{2014}'));
        assert!(EXPECT_NEEDS_VALUE.contains('\u{2019}'));
        assert!(ONE_SHOT_APPLY.contains('\u{2019}'));
        assert!(EXPECT_EQ_FORM.contains("`=`"));
    }

    #[test]
    fn happy_path_minimal() {
        let got =
            parse_apply_file_args(&v(&["--expect-sha256", HEX, "--apply-file", "/tmp/p.json"]))
                .unwrap();
        assert_eq!(
            got,
            AutoModeSetupInvocation::ApplyFile(ApplyFileInvocation {
                request_id: None,
                apply_target: None,
                expect_sha256: Some(HEX.to_string()),
                apply_file: PathBuf::from("/tmp/p.json"),
            })
        );
    }

    #[test]
    fn happy_path_full_canonical_order() {
        let got = parse_apply_file_args(&v(&[
            "--request-id",
            UUID,
            "--apply-target",
            "user",
            "--expect-sha256",
            HEX,
            "--apply-file",
            "/tmp/p.json",
        ]))
        .unwrap();
        let AutoModeSetupInvocation::ApplyFile(inv) = got else {
            panic!("expected ApplyFile");
        };
        assert_eq!(inv.request_id.as_deref(), Some(UUID));
        assert_eq!(inv.apply_target, Some(ApplyTarget::User));
        assert_eq!(inv.expect_sha256.as_deref(), Some(HEX));
    }

    #[test]
    fn apply_target_accepts_eq_and_project() {
        let AutoModeSetupInvocation::ApplyFile(inv) = parse_apply_file_args(&v(&[
            "--apply-target=project",
            "--expect-sha256",
            HEX,
            "--apply-file",
            "/p",
        ]))
        .unwrap() else {
            panic!()
        };
        assert_eq!(inv.apply_target, Some(ApplyTarget::Project));
    }

    #[test]
    fn help_flag() {
        assert_eq!(
            parse_apply_file_args(&v(&["--help"])).unwrap(),
            AutoModeSetupInvocation::Help
        );
        assert_eq!(
            parse_apply_file_args(&v(&["-h"])).unwrap(),
            AutoModeSetupInvocation::Help
        );
    }

    fn err(args: &[&str]) -> GrammarError {
        parse_apply_file_args(&v(args)).unwrap_err()
    }

    #[test]
    fn bad_apply_target_value() {
        assert_eq!(
            err(&["--apply-target", "global", "--apply-file", "/p"]).message,
            APPLY_TARGET_BAD_VALUE
        );
    }

    #[test]
    fn expect_eq_form_rejected() {
        assert_eq!(
            err(&["--expect-sha256=abc", "--apply-file", "/p"]).message,
            EXPECT_EQ_FORM
        );
    }

    #[test]
    fn expect_missing_value() {
        assert_eq!(err(&["--expect-sha256"]).message, EXPECT_NEEDS_VALUE);
    }

    #[test]
    fn expect_not_directly_before_apply_file() {
        // --expect-sha256 followed by --apply-target, not --apply-file.
        assert_eq!(
            err(&[
                "--expect-sha256",
                HEX,
                "--apply-target",
                "user",
                "--apply-file",
                "/p"
            ])
            .message,
            EXPECT_ONLY_APPLY_FILE
        );
        // --expect-sha256 with no --apply-file at all.
        assert_eq!(
            err(&["--expect-sha256", HEX]).message,
            EXPECT_ONLY_APPLY_FILE
        );
    }

    #[test]
    fn expect_after_apply_file() {
        assert_eq!(
            err(&["--apply-file", "/p", "--expect-sha256", HEX]).message,
            EXPECT_AFTER_APPLY_FILE
        );
    }

    #[test]
    fn apply_file_needs_path() {
        assert_eq!(
            err(&["--expect-sha256", HEX, "--apply-file"]).message,
            APPLY_FILE_NEEDS_PATH
        );
    }

    #[test]
    fn request_id_ordering() {
        assert_eq!(
            err(&[
                "--request-id",
                UUID,
                "--request-id",
                UUID,
                "--apply-file",
                "/p"
            ])
            .message,
            REQUEST_ID_TWICE
        );
        assert_eq!(
            err(&[
                "--apply-target",
                "user",
                "--request-id",
                UUID,
                "--apply-file",
                "/p"
            ])
            .message,
            REQUEST_ID_MUST_COME_FIRST
        );
        assert_eq!(
            err(&["--apply-file", "/p", "--request-id", UUID]).message,
            REQUEST_ID_AFTER_APPLY_FILE
        );
    }

    #[test]
    fn apply_target_ordering() {
        assert_eq!(
            err(&[
                "--apply-target",
                "user",
                "--apply-target",
                "project",
                "--apply-file",
                "/p"
            ])
            .message,
            APPLY_TARGET_TWICE
        );
        assert_eq!(
            err(&["--apply-file", "/p", "--apply-target", "user"]).message,
            APPLY_TARGET_AFTER_APPLY_FILE
        );
        // --apply-target with no --apply-file.
        assert_eq!(
            err(&["--apply-target", "user"]).message,
            APPLY_TARGET_ONLY_APPLY_FILE
        );
    }

    #[test]
    fn bare_apply_rejected_as_one_shot() {
        let e = err(&["--apply", "/p"]);
        assert_eq!(e.code, CODE_USAGE);
        assert_eq!(e.message, ONE_SHOT_APPLY);
    }

    #[test]
    fn request_id_requires_a_value() {
        assert_eq!(err(&["--request-id"]).message, REQUEST_ID_NEEDS_VALUE);
        assert_eq!(err(&["--request-id="]).message, REQUEST_ID_NEEDS_VALUE);
        // A following flag is not swallowed as the value.
        assert_eq!(
            err(&["--request-id", "--apply-file", "/p"]).message,
            REQUEST_ID_NEEDS_VALUE
        );
    }

    #[test]
    fn request_id_must_be_a_canonical_uuid() {
        assert!(is_canonical_uuid(UUID));
        assert!(is_canonical_uuid(&UUID.to_uppercase()));
        for bad in [
            "req-42",
            "3f2504e04f8911d39a0c0305e82c3301",      // no dashes
            "3f2504e0-4f89-11d3-9a0c-0305e82c330",   // too short
            "3f2504e0-4f89-11d3-9a0c-0305e82c33011", // too long
            "3f2504e0_4f89_11d3_9a0c_0305e82c3301",  // wrong separators
            "3f2504e0-4f89-11d3-9a0c-0305e82c330g",  // non-hex
        ] {
            assert!(!is_canonical_uuid(bad), "should be rejected: {bad}");
            assert_eq!(
                err(&["--request-id", bad, "--apply-file", "/p"]).message,
                REQUEST_ID_NOT_UUID
            );
        }
    }

    #[test]
    fn a_rejected_request_id_is_never_echoed_back() {
        // The message promises "the token is refused, not echoed" -- the id
        // reaches telemetry and logs, so echoing it would make this a
        // log-injection sink.
        let hostile = "\u{1b}[2K\rINFO: granted --apply-target=user";
        let e = err(&["--request-id", hostile, "--apply-file", "/p"]);
        assert_eq!(e.message, REQUEST_ID_NOT_UUID);
        assert!(!e.message.contains(hostile));
        assert!(!e.message.contains("granted"));
        assert!(!e.message.contains('\u{1b}'));
    }

    #[test]
    fn expect_sha256_does_not_swallow_a_following_flag() {
        // The oracle guards with `d.startsWith("--")`; taking the flag as the
        // digest reports an ordering error for what is really a missing value.
        assert_eq!(
            err(&["--expect-sha256", "--apply-file", "/p"]).message,
            EXPECT_NEEDS_VALUE
        );
        assert_eq!(err(&["--expect-sha256"]).message, EXPECT_NEEDS_VALUE);
        // A real digest still parses.
        let got =
            parse_apply_file_args(&v(&["--expect-sha256", HEX, "--apply-file", "/p"])).unwrap();
        let AutoModeSetupInvocation::ApplyFile(inv) = got else {
            panic!("expected ApplyFile");
        };
        assert_eq!(inv.expect_sha256.as_deref(), Some(HEX));
    }

    #[tokio::test]
    async fn containment_roots_accept_a_canonicalized_temp_path() {
        // On macOS temp_dir() is /var/folders/... while /var symlinks to
        // /private/var, so the realpath'd spelling must still be contained.
        let temp = std::env::temp_dir();
        let Ok(canonical) = std::fs::canonicalize(&temp) else {
            return;
        };
        let mut roots = vec![temp.clone()];
        for root in roots.clone() {
            if let Ok(c) = std::fs::canonicalize(&root) {
                if !roots.contains(&c) {
                    roots.push(c);
                }
            }
        }
        let probe = canonical.join("wizard06-canonical-probe.json");
        assert!(
            pipeline::path_under_containment_root(&probe, &roots),
            "canonicalized temp path {probe:?} must be contained by roots {roots:?}"
        );
    }

    #[test]
    fn propose_form_parses_the_three_answers() {
        let got = parse_apply_file_args(&v(&[
            "--wizard",
            "posture=enterprise",
            "scope=all",
            "depth=both",
            "--propose",
        ]))
        .unwrap();
        assert_eq!(
            got,
            AutoModeSetupInvocation::Propose(ProposeInvocation {
                posture: "enterprise".into(),
                scope: "all".into(),
                depth: "both".into(),
                request_id: None,
            })
        );
    }

    #[test]
    fn propose_form_rejects_answers_the_wizard_never_offers() {
        // These answers authorise how far the recon reaches, so an unoffered
        // value must fail rather than pass through.
        for args in [
            [
                "--wizard",
                "posture=root",
                "scope=all",
                "depth=both",
                "--propose",
            ],
            [
                "--wizard",
                "posture=mixed",
                "scope=everything",
                "depth=both",
                "--propose",
            ],
            [
                "--wizard",
                "posture=mixed",
                "scope=all",
                "depth=everywhere",
                "--propose",
            ],
            [
                "--wizard",
                "posture=",
                "scope=all",
                "depth=both",
                "--propose",
            ],
        ] {
            let e = err(&args);
            assert_eq!(e.code, CODE_USAGE);
            assert_eq!(e.message, PARSE_FALLBACK);
        }
    }

    #[test]
    fn propose_form_requires_the_exact_shape() {
        // Wrong order, missing --propose, or extra tokens: not the propose form,
        // so it falls through to the apply-path walk and is refused there.
        for args in [
            &[
                "--wizard",
                "scope=all",
                "posture=mixed",
                "depth=both",
                "--propose",
            ][..],
            &["--wizard", "posture=mixed", "scope=all", "depth=both"][..],
            &[
                "--wizard",
                "posture=mixed",
                "scope=all",
                "depth=both",
                "--propose",
                "x",
            ][..],
            &["--propose"][..],
        ] {
            assert!(parse_apply_file_args(&v(args)).is_err());
        }
    }

    #[test]
    fn propose_grammar_string_is_byte_exact() {
        assert_eq!(
            PROPOSE_GRAMMAR,
            r"^--wizard posture=(\S+) scope=(\S+) depth=(\S+)\s+--propose$"
        );
        assert_eq!(
            POSTURE_VALUES,
            ["personal", "open-source", "enterprise", "mixed"]
        );
        assert_eq!(SCOPE_VALUES, ["all", "project"]);
        assert_eq!(DEPTH_VALUES, ["both", "shell", "repos", "here"]);
    }

    #[test]
    fn new_grammar_messages_are_byte_exact() {
        assert_eq!(REQUEST_ID_NEEDS_VALUE, "--request-id needs a value.");
        assert_eq!(
            REQUEST_ID_NOT_UUID,
            "--request-id must be a UUID in canonical 8-4-4-4-12 hex-and-dash form (either case) \u{2014} the token is refused, not echoed."
        );
        assert_eq!(PARSE_FALLBACK, "Couldn\u{2019}t parse arguments.");
    }

    #[test]
    fn unknown_flag_falls_back_to_usage() {
        let e = err(&["--frobnicate"]);
        assert_eq!(e.code, CODE_USAGE);
        assert_eq!(e.message, PARSE_FALLBACK);
    }

    // ── `--request-id` in front of the propose form (oracle `dNd` → `Gay`) ────

    /// The propose tokens, without any leading `--request-id`.
    const PROPOSE_TAIL: [&str; 5] = [
        "--wizard",
        "posture=personal",
        "scope=project",
        "depth=here",
        "--propose",
    ];

    fn propose_with(prefix: &[&str]) -> Result<AutoModeSetupInvocation, GrammarError> {
        let mut args: Vec<String> = prefix.iter().map(|s| (*s).to_string()).collect();
        args.extend(PROPOSE_TAIL.iter().map(|s| (*s).to_string()));
        parse_apply_file_args(&args)
    }

    #[test]
    fn request_id_may_precede_the_propose_form() {
        // The oracle strips `--request-id` before matching either form, so this
        // is a valid propose invocation — not an unknown-flag rejection.
        let got = propose_with(&["--request-id", UUID]).unwrap();
        match got {
            AutoModeSetupInvocation::Propose(inv) => {
                assert_eq!(inv.request_id.as_deref(), Some(UUID));
                assert_eq!(inv.posture, "personal");
                assert_eq!(inv.depth, "here");
            }
            other => panic!("expected propose, got {other:?}"),
        }
    }

    #[test]
    fn request_id_inline_form_also_precedes_propose() {
        let got = propose_with(&[&format!("--request-id={UUID}")]).unwrap();
        match got {
            AutoModeSetupInvocation::Propose(inv) => {
                assert_eq!(inv.request_id.as_deref(), Some(UUID));
            }
            other => panic!("expected propose, got {other:?}"),
        }
    }

    #[test]
    fn propose_without_request_id_carries_none() {
        let got = propose_with(&[]).unwrap();
        match got {
            AutoModeSetupInvocation::Propose(inv) => assert!(inv.request_id.is_none()),
            other => panic!("expected propose, got {other:?}"),
        }
    }

    #[test]
    fn a_non_uuid_request_id_is_refused_without_being_echoed() {
        let e = propose_with(&["--request-id", "not-a-uuid"]).unwrap_err();
        assert_eq!(e.code, CODE_BAD_FLAG_GRAMMAR);
        assert_eq!(e.message, REQUEST_ID_NOT_UUID);
        // The whole point of the fixed string: the rejected token must not
        // reach the output, where it would land in logs.
        assert!(!e.message.contains("not-a-uuid"));
    }

    #[test]
    fn request_id_with_no_value_before_propose_is_refused() {
        let e = propose_with(&["--request-id"]).unwrap_err();
        assert_eq!(e.message, REQUEST_ID_NEEDS_VALUE);
    }

    // ── the propose result envelope (oracle `zay` over `Way`'s propose arm) ──

    fn draft() -> permission::auto_mode_propose::ProposalDraft {
        permission::auto_mode_propose::ProposalDraft {
            environment: vec!["Rust monorepo".to_string()],
            allow: vec!["Bash(cargo test:*)".to_string()],
            soft_deny: vec![],
            hard_deny: vec![],
            remove_from_permissions_allow: vec![],
            notes: vec![],
            mode: "append".to_string(),
            scope: "project".to_string(),
        }
    }

    #[test]
    fn a_successful_run_reports_ok_and_the_proposal() {
        let outcome = permission::auto_mode_propose::ProposeOutcome::Ok(Box::new(
            permission::auto_mode_propose::ProposeSuccess {
                proposal: draft(),
                gathered: "## Pre-gathered recon".to_string(),
                telemetry_code: None,
            },
        ));
        let body = propose_result_body(&outcome);
        assert_eq!(body["ok"], Value::Bool(true));
        assert_eq!(body["proposal"]["allow"][0], "Bash(cargo test:*)");
        // The failure keys must be absent, not present-and-null.
        assert!(body.get("code").is_none());
        assert!(body.get("reason").is_none());
    }

    #[test]
    fn a_failed_run_reports_the_code_and_reason_in_oracle_key_order() {
        let outcome = permission::auto_mode_propose::ProposeOutcome::Failed {
            code: permission::auto_mode_propose::PROPOSE_CODE_TRUNCATED,
            reason: "cut off".to_string(),
            emit_telemetry: true,
        };
        let json = propose_result_json(propose_result_body(&outcome), None);
        // `preserve_order` keeps insertion order, so the bytes match `zay`'s
        // `{ok, code, reason}` rather than an alphabetised map.
        assert_eq!(
            json,
            "{\n  \"ok\": false,\n  \"code\": \"truncated\",\n  \"reason\": \"cut off\"\n}"
        );
    }

    #[test]
    fn the_request_id_is_echoed_last() {
        let outcome = permission::auto_mode_propose::ProposeOutcome::Failed {
            code: permission::auto_mode_propose::PROPOSE_CODE_ABORTED,
            reason: "Cancelled.".to_string(),
            emit_telemetry: false,
        };
        let json = propose_result_json(propose_result_body(&outcome), Some(UUID));
        assert!(json.ends_with(&format!("\"requestId\": \"{UUID}\"\n}}")));
    }

    #[test]
    fn no_request_id_means_no_request_id_key() {
        let outcome = permission::auto_mode_propose::ProposeOutcome::Ok(Box::new(
            permission::auto_mode_propose::ProposeSuccess {
                proposal: draft(),
                gathered: String::new(),
                telemetry_code: None,
            },
        ));
        let json = propose_result_json(propose_result_body(&outcome), None);
        assert!(!json.contains("requestId"));
    }

    #[test]
    fn peeling_leaves_the_apply_path_grammar_untouched() {
        // A leading `--request-id` whose remainder is NOT the propose form must
        // still flow through the walk, which owns the apply-path diagnostics.
        let e = err(&["--request-id", UUID, "--apply-target", "user"]);
        assert_eq!(e.message, APPLY_TARGET_ONLY_APPLY_FILE);
    }

    // ── orchestration core (real tempfiles + real settings write) ────────────

    use serde_json::json;

    /// Write `proposal` as a real file under a fresh temp dir; return the temp
    /// dir, the file path, its containment root, and the bytes' sha256.
    fn stage_proposal(proposal: &Value) -> (tempfile::TempDir, PathBuf, PathBuf, String) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let path = root.join("proposal.json");
        let bytes = serde_json::to_vec(proposal).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let sha = pipeline::sha256_hex(&bytes);
        (dir, path, root, sha)
    }

    fn paths_under(dir: &Path) -> PermissionPaths {
        PermissionPaths {
            lingxi_home: dir.join("home/.lingxi"),
            cwd: dir.join("proj"),
        }
    }

    fn inv(path: &Path, sha: Option<&str>) -> ApplyFileInvocation {
        ApplyFileInvocation {
            request_id: None,
            apply_target: None,
            expect_sha256: sha.map(String::from),
            apply_file: path.to_path_buf(),
        }
    }

    #[tokio::test]
    async fn applies_valid_proposal_and_writes_settings() {
        let proposal = json!({
            "environment": ["Solo dev on a laptop"],
            "allow": ["Bash(ls:*)", "$defaults"],
        });
        let (dir, path, root, sha) = stage_proposal(&proposal);
        let paths = paths_under(dir.path());
        let got = execute_apply_file(&inv(&path, Some(&sha)), &[root], |_| false, &paths)
            .await
            .unwrap();
        assert_eq!(got, ApplyResult::Wrote { removed_count: 0 });
        let settings =
            std::fs::read_to_string(dir.path().join("home/.lingxi/settings.json")).unwrap();
        let v: Value = serde_json::from_str(&settings).unwrap();
        assert_eq!(
            v["autoMode"]["environment"],
            json!(["Solo dev on a laptop"])
        );
        // The rule array is merged, so `$defaults` leads it.
        assert_eq!(v["autoMode"]["allow"], json!(["$defaults", "Bash(ls:*)"]));
    }

    #[tokio::test]
    async fn append_is_the_default_and_preserves_existing_configuration() {
        // A proposal with no `mode` must NOT clobber what the user already has.
        let proposal = json!({
            "environment": ["### Org-wide", "**Organization**: acme"],
            "hard_deny": ["Bash(rm:*)", "$defaults"],
        });
        let (dir, path, root, sha) = stage_proposal(&proposal);
        let paths = paths_under(dir.path());
        let settings_path = dir.path().join("home/.lingxi/settings.json");
        std::fs::create_dir_all(settings_path.parent().unwrap()).unwrap();
        std::fs::write(
            &settings_path,
            serde_json::to_string_pretty(&json!({
                "autoMode": {
                    "environment": ["### Org-wide", "**Source control**: github"],
                    "hard_deny": ["$defaults", "Bash(dd:*)"],
                    "soft_deny": ["$defaults", "Bash(kubectl:*)"],
                }
            }))
            .unwrap(),
        )
        .unwrap();

        execute_apply_file(&inv(&path, Some(&sha)), &[root], |_| false, &paths)
            .await
            .unwrap();
        let v: Value =
            serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();

        // The prior environment bullet survives, and the new one joins its section.
        assert_eq!(
            v["autoMode"]["environment"],
            json!([
                "### Org-wide",
                "**Source control**: github",
                "**Organization**: acme"
            ])
        );
        // The prior hard_deny entry survives alongside the proposed one...
        assert_eq!(
            v["autoMode"]["hard_deny"],
            json!(["$defaults", "Bash(dd:*)", "Bash(rm:*)"])
        );
        // ...and a category the proposal never mentioned is untouched.
        assert_eq!(
            v["autoMode"]["soft_deny"],
            json!(["$defaults", "Bash(kubectl:*)"])
        );
    }

    #[tokio::test]
    async fn replace_mode_replaces_only_the_environment_section() {
        let proposal = json!({
            "environment": ["**Organization**: acme"],
            "hard_deny": ["Bash(rm:*)", "$defaults"],
            "mode": "replace",
        });
        let (dir, path, root, sha) = stage_proposal(&proposal);
        let paths = paths_under(dir.path());
        let settings_path = dir.path().join("home/.lingxi/settings.json");
        std::fs::create_dir_all(settings_path.parent().unwrap()).unwrap();
        std::fs::write(
            &settings_path,
            serde_json::to_string_pretty(&json!({
                "autoMode": {
                    "environment": ["**Source control**: github"],
                    "hard_deny": ["$defaults", "Bash(dd:*)"],
                }
            }))
            .unwrap(),
        )
        .unwrap();

        execute_apply_file(&inv(&path, Some(&sha)), &[root], |_| false, &paths)
            .await
            .unwrap();
        let v: Value =
            serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();

        // environment replaced wholesale...
        assert_eq!(
            v["autoMode"]["environment"],
            json!(["**Organization**: acme"])
        );
        // ...but the rule arrays still merge, per the wizard's own answer label
        // ("replaces the environment section").
        assert_eq!(
            v["autoMode"]["hard_deny"],
            json!(["$defaults", "Bash(dd:*)", "Bash(rm:*)"])
        );
    }

    #[tokio::test]
    async fn hash_mismatch_is_rejected_and_nothing_written() {
        let (dir, path, root, _sha) = stage_proposal(&json!({"environment": ["x"]}));
        let paths = paths_under(dir.path());
        let wrong = "f".repeat(64);
        let got = execute_apply_file(&inv(&path, Some(&wrong)), &[root], |_| false, &paths)
            .await
            .unwrap();
        let ApplyResult::Rejected { code, .. } = got else {
            panic!("expected Rejected, got {got:?}");
        };
        assert_eq!(code, "hash_mismatch");
        assert!(!dir.path().join("home/.lingxi/settings.json").exists());
    }

    #[tokio::test]
    async fn missing_file_is_read_failed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nope.json");
        let paths = paths_under(dir.path());
        let got = execute_apply_file(
            &inv(&path, Some(&"0".repeat(64))),
            &[dir.path().to_path_buf()],
            |_| false,
            &paths,
        )
        .await
        .unwrap();
        assert!(matches!(got, ApplyResult::Rejected { code, .. } if code == "read_failed"));
    }

    #[tokio::test]
    async fn non_proposal_json_is_parse_failed() {
        // Valid bytes + correct hash, but not a JSON object → parse_failed.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.json");
        let bytes = b"[1, 2, 3]";
        std::fs::write(&path, bytes).unwrap();
        let sha = pipeline::sha256_hex(bytes);
        let paths = paths_under(dir.path());
        let got = execute_apply_file(
            &inv(&path, Some(&sha)),
            &[dir.path().to_path_buf()],
            |_| false,
            &paths,
        )
        .await
        .unwrap();
        assert!(matches!(got, ApplyResult::Rejected { code, .. } if code == "parse_failed"));
    }

    #[tokio::test]
    async fn empty_environment_is_invalid_save() {
        let (dir, path, root, sha) = stage_proposal(&json!({"environment": []}));
        let paths = paths_under(dir.path());
        let got = execute_apply_file(&inv(&path, Some(&sha)), &[root], |_| false, &paths)
            .await
            .unwrap();
        assert_eq!(
            got,
            ApplyResult::InvalidSave {
                reason: "autoMode.environment is empty \u{2014} nothing to save.".to_string()
            }
        );
    }

    #[tokio::test]
    async fn removes_seeded_allow_rule() {
        use permission::{
            replace_permission_rules, PermissionBehavior, PermissionRule, PermissionRuleSource,
            PermissionRuleValue,
        };
        let proposal = json!({
            "environment": ["laptop"],
            "remove_from_permissions_allow": ["Bash(rm:*)"],
        });
        let (dir, path, root, sha) = stage_proposal(&proposal);
        let paths = paths_under(dir.path());
        // Seed the user settings allow list with the destructive rule.
        let allow_rule = |spec: &str| PermissionRule {
            value: PermissionRuleValue::from_rule_string(spec),
            behavior: PermissionBehavior::Allow,
            source: PermissionRuleSource::UserSettings,
        };
        let seed = [allow_rule("Bash(rm:*)"), allow_rule("Read")];
        replace_permission_rules(
            PermissionBehavior::Allow,
            &seed,
            PermissionUpdateDestination::UserSettings,
            &paths,
        )
        .await
        .unwrap();
        let got = execute_apply_file(&inv(&path, Some(&sha)), &[root], |_| false, &paths)
            .await
            .unwrap();
        assert_eq!(got, ApplyResult::Wrote { removed_count: 1 });
        let v: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("home/.lingxi/settings.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(v["permissions"]["allow"], json!(["Read"]));
    }

    // ── dispatch outcome mapping ─────────────────────────────────────────────

    #[test]
    fn grammar_error_disposition_carries_code_and_message() {
        let d = dispose_grammar(&grammar(APPLY_FILE_NEEDS_PATH));
        assert_eq!(d.exit_code, RUNTIME_ERROR);
        assert_eq!(d.telemetry_code.as_deref(), Some("bad_flag_grammar"));
        assert_eq!(d.stderr.as_deref(), Some(APPLY_FILE_NEEDS_PATH));
        // The usage fallback prints the oracle's generic parse message.
        let d = dispose_grammar(&usage_fallback());
        assert_eq!(d.telemetry_code.as_deref(), Some("usage"));
        assert_eq!(d.stderr.as_deref(), Some(PARSE_FALLBACK));
        assert_eq!(d.exit_code, RUNTIME_ERROR);
    }

    #[test]
    fn apply_result_dispositions() {
        // Success outcomes: exit 0, no telemetry code, no stderr.
        for ok in [
            ApplyResult::Wrote { removed_count: 3 },
            ApplyResult::NoChange,
        ] {
            let d = dispose_apply(&ok);
            assert_eq!(d.exit_code, SUCCESS);
            assert_eq!(d.telemetry_code, None);
            assert_eq!(d.stderr, None);
        }
        // Gate rejection: pipeline's own code + reason.
        let d = dispose_apply(&ApplyResult::Rejected {
            code: "hash_mismatch".into(),
            reason: "boom".into(),
        });
        assert_eq!(d.exit_code, RUNTIME_ERROR);
        assert_eq!(d.telemetry_code.as_deref(), Some("hash_mismatch"));
        assert_eq!(d.stderr.as_deref(), Some("boom"));
        // Validation failure folds into `usage`.
        let d = dispose_apply(&ApplyResult::InvalidSave {
            reason: "Nothing to save.".into(),
        });
        assert_eq!(d.telemetry_code.as_deref(), Some("usage"));
        assert_eq!(d.stderr.as_deref(), Some("Nothing to save."));
    }

    #[tokio::test]
    async fn run_help_prints_usage_and_succeeds() {
        let code = run(&Cli {
            args: vec!["--help".to_string()],
        })
        .await;
        assert_eq!(code, SUCCESS);
    }
}
