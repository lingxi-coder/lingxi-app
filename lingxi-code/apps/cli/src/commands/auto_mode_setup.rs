//! `lingxi-cli auto-mode-setup` — the WIZARD-06 permission-hardening wizard's
//! CLI arg-grammar layer (byte-parity with claude-code 2.1.218
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
//! 2.1.218 binary. Ordering precedence uses a left-to-right, first-violation-
//! wins walk over the canonical flag order (`--request-id` < `--apply-target` <
//! `--expect-sha256` < `--apply-file`); each specific message is emitted ONLY in
//! the structural situation its text describes, and flag arrangements the
//! binary's strings don't unambiguously attribute fall back to a bare `usage`
//! error (the CLI prints help) rather than a guessed message. The interactive
//! `--propose` / `--wizard` product path and the dispatch/telemetry wiring are
//! later waves that build on this parser.

use std::path::{Path, PathBuf};

use permission::auto_mode_setup as pipeline;
use permission::{
    persist_auto_mode_save, PermissionPaths, PermissionUpdateDestination, PersistError,
};
use serde_json::Value;

// ── telemetry codes (auto_mode_setup_write) ──────────────────────────────────

/// A generic usage / help outcome (`usage`).
pub const CODE_USAGE: &str = "usage";
/// A flag-ordering / flag-shape rejection (`bad_flag_grammar`).
pub const CODE_BAD_FLAG_GRAMMAR: &str = "bad_flag_grammar";

// ── byte-exact grammar messages (2.1.218) ────────────────────────────────────

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
        message: "",
    }
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
                // request-id value is telemetry metadata; take the inline (`=`)
                // value or the next token. No binary message covers a missing
                // value, so an empty id is accepted rather than guessing one.
                if let Some(v) = inline {
                    request_id = Some(v.to_string());
                    i += 1;
                } else if i + 1 < args.len() && !args[i + 1].starts_with("--") {
                    request_id = Some(args[i + 1].clone());
                    i += 2;
                } else {
                    request_id = Some(String::new());
                    i += 1;
                }
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
                if i + 1 >= args.len() || args[i + 1].is_empty() {
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

    let outcome = persist_auto_mode_save(
        Some(&block),
        &remove_strings,
        destination_for(inv.apply_target),
        paths,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn v(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| (*s).to_string()).collect()
    }
    const HEX: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

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
            "req-42",
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
        assert_eq!(inv.request_id.as_deref(), Some("req-42"));
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
            err(&["--request-id", "a", "--request-id", "b", "--apply-file", "/p"]).message,
            REQUEST_ID_TWICE
        );
        assert_eq!(
            err(&["--apply-target", "user", "--request-id", "a", "--apply-file", "/p"]).message,
            REQUEST_ID_MUST_COME_FIRST
        );
        assert_eq!(
            err(&["--apply-file", "/p", "--request-id", "a"]).message,
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
    fn unknown_flag_falls_back_to_usage() {
        let e = err(&["--frobnicate"]);
        assert_eq!(e.code, CODE_USAGE);
        assert_eq!(e.message, "");
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
        assert_eq!(v["autoMode"]["allow"], json!(["Bash(ls:*)", "$defaults"]));
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
}
