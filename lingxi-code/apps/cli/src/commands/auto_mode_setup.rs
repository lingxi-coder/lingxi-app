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

// ── telemetry codes (auto_mode_setup_write) ──────────────────────────────────

/// A generic usage / help outcome (`usage`).
pub const CODE_USAGE: &str = "usage";
/// A flag-ordering / flag-shape rejection (`bad_flag_grammar`).
pub const CODE_BAD_FLAG_GRAMMAR: &str = "bad_flag_grammar";

// ── byte-exact grammar messages (2.1.220) ────────────────────────────────────

/// `--apply-target` value was neither `user` nor `project`.
pub const APPLY_TARGET_BAD_VALUE: &str = "--apply-target must be \"user\" or \"project\".";
/// `--apply-target` appeared more than once.
pub const APPLY_TARGET_TWICE: &str =
    "--apply-target was given more than once \u{2014} pass exactly one.";
/// `--apply-target` was given but there is no `--apply-file` to apply to.
pub const APPLY_TARGET_ONLY_APPLY_FILE: &str = "--apply-target only applies to --apply-file.";
/// `--apply-target` appeared after `--apply-file`.
pub const APPLY_TARGET_AFTER_APPLY_FILE: &str =
    "--apply-target must come before --expect-sha256 and --apply-file \u{2014} not after --apply-file.";
/// `--request-id` appeared more than once.
pub const REQUEST_ID_TWICE: &str =
    "--request-id was given more than once \u{2014} pass exactly one, as the first flag.";
/// `--request-id` appeared after another flag (but before `--apply-file`).
pub const REQUEST_ID_MUST_COME_FIRST: &str =
    "--request-id must come first, before --apply-target and --expect-sha256.";
/// `--request-id` appeared after `--apply-file`.
pub const REQUEST_ID_AFTER_APPLY_FILE: &str =
    "--request-id must come first, before --expect-sha256 and --apply-file \u{2014} not after --apply-file.";
/// `--expect-sha256=<v>` (`=` form) instead of a space-separated value.
pub const EXPECT_EQ_FORM: &str =
    "--expect-sha256 takes its value space-separated, not with `=`: --expect-sha256 <64-hex> --apply-file <path>.";
/// `--expect-sha256` with no following value token.
pub const EXPECT_NEEDS_VALUE: &str =
    "--expect-sha256 needs the 64-character hex sha256 of the proposal file\u{2019}s exact bytes.";
/// `--expect-sha256` not immediately before `--apply-file` (or no `--apply-file`).
pub const EXPECT_ONLY_APPLY_FILE: &str =
    "--expect-sha256 applies only to --apply-file and must come directly before it (--apply-target goes before --expect-sha256).";
/// `--expect-sha256` appeared after `--apply-file`.
pub const EXPECT_AFTER_APPLY_FILE: &str =
    "--expect-sha256 must come before --apply-file, not after it.";
/// `--apply-file` with no path.
pub const APPLY_FILE_NEEDS_PATH: &str = "--apply-file needs a path to the reviewed proposal JSON.";
/// Bare `--apply` (one-shot apply of model output with no review) is refused.
pub const ONE_SHOT_APPLY: &str =
    "One-shot --apply isn\u{2019}t available (it would write model output with no review). Use --propose, show the result to the user, then --apply-file <path>.";

// ── parsed result ────────────────────────────────────────────────────────────

/// The `--apply-target` save scope (`YQ_[target]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyTarget {
    /// `user` — the user settings tier.
    User,
    /// `project` — the project settings tier.
    Project,
}

impl ApplyTarget {
    /// The verbatim flag value (`"user"` / `"project"`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ApplyTarget::User => "user",
            ApplyTarget::Project => "project",
        }
    }
}

/// `--request-id` was given with no value (`--request-id=` or followed by
/// another flag).
pub const REQUEST_ID_NEEDS_VALUE: &str = "--request-id needs a value.";
/// `--request-id` was given a value that is not a canonical UUID.
///
/// Note what the wording commits to: "the token is refused, not echoed". The
/// rejected id must NOT appear in the error output. A request id lands in
/// telemetry and logs, so echoing an arbitrary token back would turn this
/// message into a log-injection sink — hence a fixed string with no
/// interpolation slot.
pub const REQUEST_ID_NOT_UUID: &str = "--request-id must be a UUID in canonical 8-4-4-4-12 hex-and-dash form (either case) \u{2014} the token is refused, not echoed.";

/// Printed when no specific message attributes the flag arrangement.
pub const PARSE_FALLBACK: &str = "Couldn\u{2019}t parse arguments.";

/// The non-interactive propose entry point's grammar, as the oracle spells it.
/// The whole argument string must match this exactly.
pub const PROPOSE_GRAMMAR: &str = r"^--wizard posture=(\S+) scope=(\S+) depth=(\S+)\s+--propose$";

/// Accepted `posture=` values.
pub const POSTURE_VALUES: [&str; 4] = ["personal", "open-source", "enterprise", "mixed"];
/// Accepted `scope=` values.
pub const SCOPE_VALUES: [&str; 2] = ["all", "project"];
/// Accepted `depth=` values.
pub const DEPTH_VALUES: [&str; 4] = ["both", "shell", "repos", "here"];

/// A validated `--wizard … --propose` invocation: the three setup answers that
/// would otherwise come from the interactive questions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposeInvocation {
    /// `posture=` — one of [`POSTURE_VALUES`].
    pub posture: String,
    /// `scope=` — one of [`SCOPE_VALUES`].
    pub scope: String,
    /// `depth=` — one of [`DEPTH_VALUES`].
    pub depth: String,
}

/// Is `s` a UUID in canonical 8-4-4-4-12 hex-and-dash form (either case)?
#[must_use]
pub fn is_canonical_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 36 {
        return false;
    }
    b.iter().enumerate().all(|(i, c)| match i {
        8 | 13 | 18 | 23 => *c == b'-',
        _ => c.is_ascii_hexdigit(),
    })
}

/// A fully order-validated `--apply-file` invocation. `expect_sha256` is left
/// optional here: a MISSING hash is rejected downstream by the permission
/// pipeline's `missing_hash_arg` gate, keeping the hash-bind enforcement in one
/// place regardless of grammar leniency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyFileInvocation {
    /// `--request-id` (telemetry correlation id), if any.
    pub request_id: Option<String>,
    /// `--apply-target` scope, if any.
    pub apply_target: Option<ApplyTarget>,
    /// `--expect-sha256` value (still validated for 64-hex downstream).
    pub expect_sha256: Option<String>,
    /// `--apply-file` proposal path.
    pub apply_file: PathBuf,
}

/// The result of parsing `auto-mode-setup` flags for the apply path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutoModeSetupInvocation {
    /// `--help` / `-h` — print usage.
    Help,
    /// A valid `--apply-file` invocation ready for the permission pipeline.
    ApplyFile(ApplyFileInvocation),
    /// A valid `--wizard … --propose` invocation.
    Propose(ProposeInvocation),
}

/// A grammar rejection: the byte-exact `auto_mode_setup_write` `code` +
/// user-facing `message`. An empty `message` means "no specific attributable
/// message — print usage/help".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrammarError {
    /// The telemetry code (`usage` / `bad_flag_grammar`).
    pub code: &'static str,
    /// The byte-exact message, or `""` to fall back to help.
    pub message: &'static str,
}

fn grammar(message: &'static str) -> GrammarError {
    GrammarError {
        code: CODE_BAD_FLAG_GRAMMAR,
        message,
    }
}

/// A bare `usage` fallback (caller prints help). Used both for `--help` errors
/// and for flag arrangements the binary's strings don't unambiguously attribute.
fn usage_fallback() -> GrammarError {
    GrammarError {
        code: CODE_USAGE,
        message: PARSE_FALLBACK,
    }
}

/// Match the `--wizard posture=… scope=… depth=… --propose` form.
///
/// [`PROPOSE_GRAMMAR`] anchors the WHOLE argument string, with single spaces
/// between the four leading tokens, so on a token vector that is exactly five
/// tokens in that order. Returns `None` when the shape does not match at all
/// (the caller then continues into the apply-path parser); returns
/// `Some(Err(..))` when the shape matches but an answer value is not one the
/// wizard offers.
fn match_propose_form(args: &[String]) -> Option<Result<ProposeInvocation, GrammarError>> {
    if args.len() != 5 || args[0] != "--wizard" || args[4] != "--propose" {
        return None;
    }
    let posture = args[1].strip_prefix("posture=")?;
    let scope = args[2].strip_prefix("scope=")?;
    let depth = args[3].strip_prefix("depth=")?;
    // `(\S+)` — a value may not be empty or contain whitespace.
    if [posture, scope, depth]
        .iter()
        .any(|v| v.is_empty() || v.split_whitespace().count() != 1)
    {
        return Some(Err(usage_fallback()));
    }
    // Fail closed on an unoffered answer rather than passing it through: these
    // values authorise how far the recon reaches.
    if !POSTURE_VALUES.contains(&posture)
        || !SCOPE_VALUES.contains(&scope)
        || !DEPTH_VALUES.contains(&depth)
    {
        return Some(Err(usage_fallback()));
    }
    Some(Ok(ProposeInvocation {
        posture: posture.to_string(),
        scope: scope.to_string(),
        depth: depth.to_string(),
    }))
}

/// Split `--flag=value` into (`--flag`, `Some(value)`); a bare flag is
/// (`--flag`, `None`).
fn split_eq(token: &str) -> (&str, Option<&str>) {
    if token.starts_with("--") {
        if let Some(eq) = token.find('=') {
            return (&token[..eq], Some(&token[eq + 1..]));
        }
    }
    (token, None)
}

/// Parse + order-validate `auto-mode-setup` apply-path flags (`args` are the
/// tokens after the subcommand name). See the module docs for the precedence
/// model. Returns [`AutoModeSetupInvocation`] on success or a byte-exact
/// [`GrammarError`].
///
/// # Errors
/// Returns [`GrammarError`] for any flag-shape or ordering violation.
pub fn parse_apply_file_args(args: &[String]) -> Result<AutoModeSetupInvocation, GrammarError> {
    // The propose form is a whole-string grammar of its own, checked before the
    // flag walk so its tokens are never mistaken for apply-path flags.
    if let Some(result) = match_propose_form(args) {
        return result.map(AutoModeSetupInvocation::Propose);
    }

    let mut request_id: Option<String> = None;
    let mut apply_target: Option<ApplyTarget> = None;
    let mut expect_sha256: Option<String> = None;
    let mut apply_file: Option<String> = None;
    // `--expect-sha256` must be immediately followed by `--apply-file`.
    let mut expect_awaiting_file = false;

    let mut i = 0;
    while i < args.len() {
        let (name, inline) = split_eq(&args[i]);

        // Enforce "--expect-sha256 ... must come directly before --apply-file".
        if expect_awaiting_file && name != "--apply-file" {
            return Err(grammar(EXPECT_ONLY_APPLY_FILE));
        }

        match name {
            "--help" | "-h" => return Ok(AutoModeSetupInvocation::Help),
            "--apply" => {
                return Err(GrammarError {
                    code: CODE_USAGE,
                    message: ONE_SHOT_APPLY,
                })
            }
            "--request-id" => {
                if request_id.is_some() {
                    return Err(grammar(REQUEST_ID_TWICE));
                }
                if apply_file.is_some() {
                    return Err(grammar(REQUEST_ID_AFTER_APPLY_FILE));
                }
                if apply_target.is_some() || expect_sha256.is_some() {
                    return Err(grammar(REQUEST_ID_MUST_COME_FIRST));
                }
                // `^--request-id(?:=|\s+(?!--))(\S+)\s*` — the value is taken
                // inline after `=` or as the next token, but NOT when that token
                // is itself a flag.
                let value = if let Some(v) = inline {
                    i += 1;
                    v.to_string()
                } else if i + 1 < args.len() && !args[i + 1].starts_with("--") {
                    i += 2;
                    args[i - 1].clone()
                } else {
                    i += 1;
                    String::new()
                };
                if value.is_empty() {
                    return Err(grammar(REQUEST_ID_NEEDS_VALUE));
                }
                // The id reaches telemetry and logs, so it is constrained to a
                // canonical UUID and a bad token is refused WITHOUT being echoed.
                if !is_canonical_uuid(&value) {
                    return Err(grammar(REQUEST_ID_NOT_UUID));
                }
                request_id = Some(value);
            }
            "--apply-target" => {
                if apply_file.is_some() {
                    return Err(grammar(APPLY_TARGET_AFTER_APPLY_FILE));
                }
                if apply_target.is_some() {
                    return Err(grammar(APPLY_TARGET_TWICE));
                }
                if expect_sha256.is_some() {
                    // After --expect-sha256 but before --apply-file: no binary
                    // message attributes this exactly → usage fallback.
                    return Err(usage_fallback());
                }
                let (value, next_i) = if let Some(v) = inline {
                    (v.to_string(), i + 1)
                } else if i + 1 < args.len() {
                    (args[i + 1].clone(), i + 2)
                } else {
                    (String::new(), i + 1)
                };
                apply_target = Some(match value.as_str() {
                    "user" => ApplyTarget::User,
                    "project" => ApplyTarget::Project,
                    _ => return Err(grammar(APPLY_TARGET_BAD_VALUE)),
                });
                i = next_i;
            }
            "--expect-sha256" => {
                if apply_file.is_some() {
                    return Err(grammar(EXPECT_AFTER_APPLY_FILE));
                }
                if expect_sha256.is_some() {
                    // Repeated --expect-sha256: no specific binary message.
                    return Err(usage_fallback());
                }
                if inline.is_some() {
                    return Err(grammar(EXPECT_EQ_FORM));
                }
                // `d === void 0 || d.startsWith("--")` — a following FLAG is not
                // taken as the digest. Without this the flag is consumed as the
                // hash and the walk reports a misleading ordering error instead
                // of "needs a value".
                if i + 1 >= args.len()
                    || args[i + 1].is_empty()
                    || args[i + 1].starts_with("--")
                {
                    return Err(grammar(EXPECT_NEEDS_VALUE));
                }
                expect_sha256 = Some(args[i + 1].clone());
                expect_awaiting_file = true;
                i += 2;
            }
            "--apply-file" => {
                let path = if let Some(v) = inline {
                    let p = v.to_string();
                    i += 1;
                    p
                } else if i + 1 < args.len() && !args[i + 1].is_empty() {
                    let p = args[i + 1].clone();
                    i += 2;
                    p
                } else {
                    return Err(grammar(APPLY_FILE_NEEDS_PATH));
                };
                if path.is_empty() {
                    return Err(grammar(APPLY_FILE_NEEDS_PATH));
                }
                apply_file = Some(path);
                expect_awaiting_file = false;
            }
            _ => return Err(usage_fallback()),
        }
    }

    // Post-walk: --expect-sha256 with no following --apply-file.
    if expect_awaiting_file {
        return Err(grammar(EXPECT_ONLY_APPLY_FILE));
    }
    let Some(apply_file) = apply_file else {
        // No --apply-file. Attribute a dangling --apply-target / --expect-sha256.
        if apply_target.is_some() {
            return Err(grammar(APPLY_TARGET_ONLY_APPLY_FILE));
        }
        if expect_sha256.is_some() {
            return Err(grammar(EXPECT_ONLY_APPLY_FILE));
        }
        return Err(usage_fallback());
    };

    Ok(AutoModeSetupInvocation::ApplyFile(ApplyFileInvocation {
        request_id,
        apply_target,
        expect_sha256,
        apply_file: PathBuf::from(apply_file),
    }))
}

// ── apply-file orchestration core ────────────────────────────────────────────

/// The result of running the `--apply-file` flow end-to-end. Gate rejections
/// carry the permission pipeline's OWN byte-exact `code` + `reason`; a
/// save-payload validation failure carries the byte-exact `vNs` reason (its
/// telemetry code is confirmed in the dispatch wave, so it is kept distinct
/// here rather than guessed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyResult {
    /// A pre-write gate rejected the request (`bad_path` / `read_denied` /
    /// `read_failed` / `too_large` / `missing_hash_arg` / `bad_hash_arg` /
    /// `hash_mismatch` / `parse_failed` / `scope_mismatch`).
    Rejected {
        /// The byte-exact `auto_mode_setup_write` code.
        code: String,
        /// The byte-exact user-facing reason.
        reason: String,
    },
    /// [`pipeline::validate_auto_mode_save`] rejected the payload; `reason` is
    /// byte-exact (e.g. "Nothing to save.").
    InvalidSave {
        /// The byte-exact `vNs` validation message.
        reason: String,
    },
    /// The settings file already matched — nothing was written.
    NoChange,
    /// The settings were written; `removed_count` `permissions.allow` entries
    /// were filtered out.
    Wrote {
        /// How many `permissions.allow` rules the removal set filtered out.
        removed_count: usize,
    },
}

/// Map `--apply-target` to the destination settings tier (default: user).
fn destination_for(target: Option<ApplyTarget>) -> PermissionUpdateDestination {
    match target {
        Some(ApplyTarget::Project) => PermissionUpdateDestination::ProjectSettings,
        _ => PermissionUpdateDestination::UserSettings,
    }
}

/// Read one proposal category (`environment` / `allow` / `soft_deny` /
/// `hard_deny`) as an array of values (empty when absent or non-array).
fn category(proposal: &Value, key: &str) -> Vec<Value> {
    proposal
        .get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Run the `--apply-file` flow: secure read → pre-write gates
/// ([`pipeline::evaluate_apply_file`]) → build the `autoMode` block from the
/// proposal → validate the save payload → persist. The security-relevant gates
/// (path/read, hash-verify, parse, scope) all run inside `evaluate_apply_file`;
/// this function only sequences them and maps the proposal to the write.
///
/// `roots` are the temp/config containment roots, `is_read_denied` the live
/// policy's `Read`-deny predicate, `paths` the settings destination roots.
///
/// # Errors
/// Propagates [`PersistError`] from the settings write (broken-JSON destination
/// or a hardened-filesystem failure).
pub async fn execute_apply_file(
    inv: &ApplyFileInvocation,
    roots: &[PathBuf],
    is_read_denied: impl Fn(&Path) -> bool,
    paths: &PermissionPaths,
) -> Result<ApplyResult, PersistError> {
    let read = pipeline::read_proposal_file(&inv.apply_file);
    let scope = inv.apply_target.map(ApplyTarget::as_str);
    let args = pipeline::ApplyFileArgs {
        path: &inv.apply_file,
        roots,
        expect_sha256: inv.expect_sha256.as_deref(),
        apply_target: scope,
        expected_scope: scope,
    };
    let proposal = match pipeline::evaluate_apply_file(&args, is_read_denied, read) {
        pipeline::ApplyFilePipeline::Rejected { code, reason } => {
            return Ok(ApplyResult::Rejected { code, reason });
        }
        pipeline::ApplyFilePipeline::Proceed { proposal } => proposal,
    };

    let block = pipeline::build_auto_mode_settings(
        &category(&proposal, "environment"),
        &category(&proposal, "allow"),
        &category(&proposal, "soft_deny"),
        &category(&proposal, "hard_deny"),
    );
    let remove_value = proposal.get("remove_from_permissions_allow").cloned();

    if let Some(reason) = pipeline::validate_auto_mode_save(Some(&block), remove_value.as_ref()) {
        return Ok(ApplyResult::InvalidSave { reason });
    }

    let remove_strings: Vec<String> = remove_value
        .as_ref()
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();

    // The proposal's own `mode` decides whether this merges with the block
    // already on disk. It defaults to `append`, so a proposal that omits the
    // key must NOT clobber the user's existing auto-mode configuration.
    let mode = pipeline::AutoModeSaveMode::from_proposal(proposal.get("mode"));

    let outcome = persist_auto_mode_save(
        Some(&block),
        &remove_strings,
        destination_for(inv.apply_target),
        paths,
        mode,
    )
    .await?;

    Ok(if outcome.wrote {
        ApplyResult::Wrote {
            removed_count: outcome.removed_count,
        }
    } else {
        ApplyResult::NoChange
    })
}

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
        // The propose grammar and its whole vocabulary are ported (see
        // `permission::auto_mode_propose` / `auto_mode_pregather`), but the
        // orchestration behind it — the recon gather plus the `json_schema`
        // model call — is not wired here yet. Fail without inventing an outcome:
        // emitting a `recon_failed` or printing a scan message would report a
        // scan that never ran.
        Ok(AutoModeSetupInvocation::Propose(_)) => RUNTIME_ERROR,
        Err(e) => apply_disposition(dispose_grammar(&e)),
    }
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
    eprintln!("Usage: lingxi-cli auto-mode-setup [--request-id <id>] [--apply-target user|project] \\");
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
        assert_eq!(APPLY_TARGET_BAD_VALUE, "--apply-target must be \"user\" or \"project\".");
        assert!(APPLY_TARGET_TWICE.contains('\u{2014}'));
        assert!(EXPECT_NEEDS_VALUE.contains('\u{2019}'));
        assert!(ONE_SHOT_APPLY.contains('\u{2019}'));
        assert!(EXPECT_EQ_FORM.contains("`=`"));
    }

    #[test]
    fn happy_path_minimal() {
        let got = parse_apply_file_args(&v(&["--expect-sha256", HEX, "--apply-file", "/tmp/p.json"]))
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
        assert_eq!(err(&["--apply-target", "global", "--apply-file", "/p"]).message, APPLY_TARGET_BAD_VALUE);
    }

    #[test]
    fn expect_eq_form_rejected() {
        assert_eq!(err(&["--expect-sha256=abc", "--apply-file", "/p"]).message, EXPECT_EQ_FORM);
    }

    #[test]
    fn expect_missing_value() {
        assert_eq!(err(&["--expect-sha256"]).message, EXPECT_NEEDS_VALUE);
    }

    #[test]
    fn expect_not_directly_before_apply_file() {
        // --expect-sha256 followed by --apply-target, not --apply-file.
        assert_eq!(
            err(&["--expect-sha256", HEX, "--apply-target", "user", "--apply-file", "/p"]).message,
            EXPECT_ONLY_APPLY_FILE
        );
        // --expect-sha256 with no --apply-file at all.
        assert_eq!(err(&["--expect-sha256", HEX]).message, EXPECT_ONLY_APPLY_FILE);
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
        assert_eq!(err(&["--expect-sha256", HEX, "--apply-file"]).message, APPLY_FILE_NEEDS_PATH);
    }

    #[test]
    fn request_id_ordering() {
        assert_eq!(
            err(&["--request-id", UUID, "--request-id", UUID, "--apply-file", "/p"]).message,
            REQUEST_ID_TWICE
        );
        assert_eq!(
            err(&["--apply-target", "user", "--request-id", UUID, "--apply-file", "/p"]).message,
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
            err(&["--apply-target", "user", "--apply-target", "project", "--apply-file", "/p"]).message,
            APPLY_TARGET_TWICE
        );
        assert_eq!(
            err(&["--apply-file", "/p", "--apply-target", "user"]).message,
            APPLY_TARGET_AFTER_APPLY_FILE
        );
        // --apply-target with no --apply-file.
        assert_eq!(err(&["--apply-target", "user"]).message, APPLY_TARGET_ONLY_APPLY_FILE);
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
            "3f2504e04f8911d39a0c0305e82c3301",          // no dashes
            "3f2504e0-4f89-11d3-9a0c-0305e82c330",       // too short
            "3f2504e0-4f89-11d3-9a0c-0305e82c33011",     // too long
            "3f2504e0_4f89_11d3_9a0c_0305e82c3301",      // wrong separators
            "3f2504e0-4f89-11d3-9a0c-0305e82c330g",      // non-hex
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
        let got = parse_apply_file_args(&v(&["--expect-sha256", HEX, "--apply-file", "/p"])).unwrap();
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
            })
        );
    }

    #[test]
    fn propose_form_rejects_answers_the_wizard_never_offers() {
        // These answers authorise how far the recon reaches, so an unoffered
        // value must fail rather than pass through.
        for args in [
            ["--wizard", "posture=root", "scope=all", "depth=both", "--propose"],
            ["--wizard", "posture=mixed", "scope=everything", "depth=both", "--propose"],
            ["--wizard", "posture=mixed", "scope=all", "depth=everywhere", "--propose"],
            ["--wizard", "posture=", "scope=all", "depth=both", "--propose"],
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
            &["--wizard", "scope=all", "posture=mixed", "depth=both", "--propose"][..],
            &["--wizard", "posture=mixed", "scope=all", "depth=both"][..],
            &["--wizard", "posture=mixed", "scope=all", "depth=both", "--propose", "x"][..],
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
        let settings = std::fs::read_to_string(dir.path().join("home/.lingxi/settings.json")).unwrap();
        let v: Value = serde_json::from_str(&settings).unwrap();
        assert_eq!(v["autoMode"]["environment"], json!(["Solo dev on a laptop"]));
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
            json!(["### Org-wide", "**Source control**: github", "**Organization**: acme"])
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
        assert_eq!(v["autoMode"]["environment"], json!(["**Organization**: acme"]));
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
        let v: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join("home/.lingxi/settings.json")).unwrap())
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
