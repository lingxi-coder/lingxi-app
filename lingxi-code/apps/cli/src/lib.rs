//! Library surface of the `lingxi-cli` crate.
//!
//! Integration tests import the public API instead of shelling out to the
//! binary so they don't need the workspace target dir hot.
//!
//! # One-shot mode
//!
//! ```text
//! $ lingxi-cli "fix the bug in foo.rs"
//! $ lingxi-cli -p "list the files in this repo"
//! $ lingxi-cli --no-stream --json "what's the weather?"
//! ```
//!
//! # Resume mode
//!
//! ```text
//! $ lingxi-cli --resume <uuid>   # load that session
//! $ lingxi-cli --resume          # TTY: iocraft Resume screen (M7-12)
//! $ lingxi-cli --resume --no-tui # stdio picker over 5 most-recent (M5-08)
//! ```
//!
//! # REPL mode (M5-13)
//!
//! Invoking `lingxi-cli` without a positional prompt drops into a
//! line-based REPL:
//!
//! ```text
//! $ lingxi-cli
//! > /version
//! lingxi-cli 0.5.0 (abc1234)
//! > hello, claude
//! Hi! How can I help?
//! > /exit
//! Exiting.
//! ```
//!
//! - **Prompt**: `"> "` printed to stderr (so stdout stays parseable in
//!   `--json` mode).
//! - **EOF (Ctrl+D)**: persists the session and exits 0.
//! - **First Ctrl+C during a turn**: cancels the turn, returns to prompt.
//! - **First Ctrl+C at idle prompt**: arms a flag; second Ctrl+C within
//!   2 seconds exits with code 130.
//! - **`/exit`**: flips the orchestrator's `should_exit` flag; REPL
//!   detects it after the dispatcher returns and exits 0.
//!
//! # Exit codes
//!
//! See [`exit_codes`].
//!
//! # Plan reference
//!
//! `docs/superpowers/plans/2026-05-25-m5-12-cli-binary.md` (one-shot),
//! `docs/superpowers/plans/2026-05-25-m5-13-repl-mode.md` (REPL).

#![forbid(unsafe_code)]

pub mod agents_notify;
pub mod agents_registry;
pub mod argv;
pub mod ax_screen_reader;
pub mod background_dispatch;
pub mod background_launch;
pub mod bg_attach;
pub mod bg_attach_stall;
pub mod bg_reply_queue;
pub mod bg_session_forker;
mod bypass_env;
pub mod commands;
pub mod control_plane;
pub mod cwd;
pub mod daemon_lock;
pub mod daemon_roster;
pub mod exit_codes;
pub mod idle_notify;
pub mod init;
pub mod logging;
pub mod mode;
pub mod output;
pub mod output_adapter;
pub mod permission_prompt_notify;
pub(crate) mod process_wrapper;
pub mod queued_commands;
pub mod repl;
pub mod repl_loop;
pub mod resume_truncation;
pub mod run;
pub mod session_cost;
pub mod sigint;
mod startup_resources;
mod startup_trace;
pub mod stream_json;
pub mod stream_json_input;
pub mod structured_output;

use crate::argv::Argv;
use clap::error::ErrorKind;
use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;

/// First missing required argument's bare name from a clap
/// `MissingRequiredArgument` error, with clap's `<…>`/`[…]`/`...` usage
/// decoration stripped — so callers can render commander's
/// `error: missing required argument '<name>'` (claude-code parity). clap lists
/// the missing args in declaration order; commander reports only the first.
fn first_missing_required_arg(e: &clap::Error) -> Option<String> {
    use clap::error::{ContextKind, ContextValue};
    let raw = match e.get(ContextKind::InvalidArg)? {
        ContextValue::Strings(v) => v.first()?.clone(),
        ContextValue::String(s) => s.clone(),
        _ => return None,
    };
    let cleaned = raw
        .trim()
        .trim_end_matches("...")
        .trim_matches(|c| c == '<' || c == '>' || c == '[' || c == ']')
        .to_string();
    (!cleaned.is_empty()).then_some(cleaned)
}

/// One value out of a clap error context slot (the first, if it is a list).
fn ctx_string(e: &clap::Error, kind: clap::error::ContextKind) -> Option<String> {
    match e.get(kind)? {
        clap::error::ContextValue::String(s) => Some(s.clone()),
        clap::error::ContextValue::Strings(v) => v.first().cloned(),
        _ => None,
    }
}

/// Check for an occupied transcript id anywhere under the current config
/// home's `projects/` store. Fresh `--session-id` launches/forks must fail
/// before runtime construction instead of appending to an existing JSONL.
///
/// A missing store is an empty store. Every other I/O failure is surfaced so
/// the caller fails closed instead of treating an unreadable store as proof
/// that the id is unused.
async fn session_id_exists_in_store(
    config_home: &Path,
    session_id: uuid::Uuid,
) -> std::io::Result<bool> {
    let projects_root = config_home.join("projects");
    let mut entries = match tokio::fs::read_dir(&projects_root).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(std::io::Error::new(
                error.kind(),
                format!("failed to read {}: {error}", projects_root.display()),
            ));
        }
    };
    let file_name = format!("{session_id}.jsonl");
    loop {
        let Some(entry) = entries.next_entry().await.map_err(|error| {
            std::io::Error::new(
                error.kind(),
                format!("failed to scan {}: {error}", projects_root.display()),
            )
        })?
        else {
            return Ok(false);
        };
        if !entry.file_type().await?.is_dir() {
            continue;
        }
        if tokio::fs::try_exists(entry.path().join(&file_name)).await? {
            return Ok(true);
        }
    }
}

/// Damerau-Levenshtein edit distance, faithful to commander's `editDistance`
/// (suggestSimilar.js): includes the transposition rule AND the early-out
/// `|len(a)-len(b)| > maxDistance ⇒ max(len)` so clap's different default
/// metric can't pick a different "Did you mean" candidate than the oracle.
fn edit_distance(a: &str, b: &str, max_distance: usize) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (la, lb) = (a.len(), b.len());
    if la.abs_diff(lb) > max_distance {
        return la.max(lb);
    }
    let mut d = vec![vec![0usize; lb + 1]; la + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for j in 0..=lb {
        d[0][j] = j;
    }
    for i in 1..=la {
        for j in 1..=lb {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut m = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                m = m.min(d[i - 2][j - 2] + 1);
            }
            d[i][j] = m;
        }
    }
    d[la][lb]
}

/// commander's `suggestSimilar`: among `candidates`, keep those with similarity
/// `(maxLen-dist)/maxLen > 0.4` at the minimum edit distance (≤ 3), sorted; emit
/// `(Did you mean X?)` for one or `(Did you mean one of A, B?)` for several.
/// `None` when nothing is close enough (commander then prints no suggestion).
fn suggest_similar(word: &str, candidates: &[String]) -> Option<String> {
    const MAX_DISTANCE: usize = 3;
    const MIN_SIMILARITY: f64 = 0.4;
    let mut seen = std::collections::HashSet::new();
    let mut best: Vec<String> = Vec::new();
    let mut best_distance = MAX_DISTANCE;
    for cand in candidates {
        if !seen.insert(cand.as_str()) || cand.chars().count() <= 1 {
            continue;
        }
        let distance = edit_distance(word, cand, MAX_DISTANCE);
        let length = word.chars().count().max(cand.chars().count());
        if length == 0 {
            continue;
        }
        let similarity = (length - distance) as f64 / length as f64;
        if similarity > MIN_SIMILARITY {
            if distance < best_distance {
                best_distance = distance;
                best = vec![cand.clone()];
            } else if distance == best_distance {
                best.push(cand.clone());
            }
        }
    }
    best.sort();
    match best.len() {
        0 => None,
        1 => Some(format!("(Did you mean {}?)", best[0])),
        _ => Some(format!("(Did you mean one of {}?)", best.join(", "))),
    }
}

/// Subcommand names valid at the point where an invalid subcommand was typed —
/// the candidate set for [`suggest_similar`]. Walks the clap command tree along
/// the subcommand tokens in `args` up to the bad token, then lists that node's
/// subcommands (matching commander's candidate set, which includes `help`).
fn invalid_subcommand_candidates(args: &[OsString], bad: &str) -> Vec<String> {
    use clap::CommandFactory;
    let mut cmd = Argv::command();
    for tok in args.iter().skip(1) {
        let t = tok.to_string_lossy();
        if t == bad {
            break;
        }
        if let Some(sub) = cmd.find_subcommand(t.as_ref()) {
            cmd = sub.clone();
        }
    }
    cmd.get_subcommands()
        .map(|s| s.get_name().to_string())
        .collect()
}

/// Reformat the clap argv errors that claude-code (commander) renders
/// differently, to commander's exact single-/two-line form (stderr, exit 1).
/// Returns `None` for kinds we leave to clap's own rendering (the remaining
/// clap-vs-commander help-block layout difference).
fn commander_error(e: &clap::Error, args: &[OsString]) -> Option<String> {
    use clap::error::{ContextKind, ErrorKind};
    match e.kind() {
        // `error: missing required argument '<name>'` (FIRST missing positional).
        ErrorKind::MissingRequiredArgument => {
            first_missing_required_arg(e).map(|n| format!("error: missing required argument '{n}'"))
        }
        // `error: unknown option '--flag'`. clap also raises `UnknownArgument`
        // for EXCESS POSITIONALS, but commander silently ignores those — so only
        // reformat when the offending token is a flag (`-`-prefixed); a bare
        // positional falls through to clap (excess-positional parity is separate).
        ErrorKind::UnknownArgument => {
            let arg = ctx_string(e, ContextKind::InvalidArg)?;
            arg.starts_with('-')
                .then(|| format!("error: unknown option '{arg}'"))
        }
        // `error: unknown command '<cmd>'` + optional `(Did you mean <x>?)`. The
        // suggestion is computed with commander's own algorithm/candidate set
        // (NOT clap's, which picks different candidates — e.g. `ad`⇒`add-json`
        // vs commander's `add`).
        ErrorKind::InvalidSubcommand => {
            let cmd = ctx_string(e, ContextKind::InvalidSubcommand)?;
            let mut msg = format!("error: unknown command '{cmd}'");
            let candidates = invalid_subcommand_candidates(args, &cmd);
            if let Some(s) = suggest_similar(&cmd, &candidates) {
                msg.push('\n');
                msg.push_str(&s);
            }
            Some(msg)
        }
        // `error: option '<flag> <placeholder>' argument '<value>' is invalid.
        // Allowed choices are <choices>.` — clap's choices (`ValidValue`) are
        // already in declared order, matching commander. clap renders an
        // optional-value placeholder as `[<x>]`; commander uses `[x]`, so strip
        // the inner angle brackets.
        ErrorKind::InvalidValue => {
            let flag = ctx_string(e, ContextKind::InvalidArg)?
                .replace("[<", "[")
                .replace(">]", "]");
            let value = ctx_string(e, ContextKind::InvalidValue)?;
            let choices = match e.get(ContextKind::ValidValue)? {
                clap::error::ContextValue::Strings(v) => v.clone(),
                clap::error::ContextValue::String(s) => vec![s.clone()],
                _ => return None,
            };
            Some(format!(
                "error: option '{flag}' argument '{value}' is invalid. Allowed choices are {}.",
                choices.join(", ")
            ))
        }
        _ => None,
    }
}

/// Normalize clap's visible-alias layout to commander's option-heading layout.
///
/// clap renders a visible alias as a detached `[aliases: ...]` paragraph while
/// commander renders both spellings in the option heading.  The aliases below
/// are part of Claude Code's public root-help contract, so keep their accepted
/// parser spellings *and* present them in the same place in `--help` output.
/// Move the `Usage:` block ahead of the description, as commander renders it.
///
/// The usage block is the `Usage:` line plus any following INDENTED
/// continuation lines (clap wraps long usage strings that way); taking only the
/// first line would strip the tail of a wrapped usage onto the wrong side of
/// the description.
fn normalise_preamble(help: &str) -> String {
    let lines: Vec<&str> = help.lines().collect();
    let Some(start) = lines.iter().position(|l| l.starts_with("Usage:")) else {
        return help.to_string();
    };
    if start == 0 {
        return help.to_string();
    }
    let mut end = start + 1;
    while end < lines.len()
        && lines[end].starts_with(char::is_whitespace)
        && !lines[end].trim().is_empty()
    {
        end += 1;
    }
    let usage = &lines[start..end];
    let before = &lines[..start];
    let after = &lines[end..];
    let mut out: Vec<&str> = Vec::with_capacity(lines.len() + 1);
    out.extend_from_slice(usage);
    out.push("");
    out.extend(before.iter().copied());
    out.extend(after.iter().copied());
    let joined = out.join("\n");
    let mut joined = joined;
    while joined.contains("\n\n\n") {
        joined = joined.replace("\n\n\n", "\n\n");
    }
    joined
}

/// Reorder clap's help sections into commander's order.
///
/// clap emits `Commands:` before `Arguments:`/`Options:`; commander emits
/// `Arguments:` -> `Options:` -> `Commands:`. This is a pure text transform on
/// the RENDERED help rather than a `help_template`, because a template must be
/// written per command: `{all-args}` cannot be reordered, and spelling the
/// sections out individually would print a bare `Arguments:` header for the
/// ~40 subcommands that have no positionals.
///
/// Only sections that are actually present move, so a command with no
/// positionals still emits no `Arguments:` header. Anything that is not one of
/// the three known sections keeps its position relative to the preamble, so an
/// unrecognised block cannot be silently dropped.
fn reorder_help_sections(help: &str) -> String {
    // A section header is a line at column 0 ending in ':' — clap's own format.
    fn is_header(line: &str) -> bool {
        !line.starts_with(char::is_whitespace)
            && line.ends_with(':')
            && line.len() > 1
            && line.starts_with(|c: char| c.is_ascii_uppercase())
    }

    // Normalise the PREAMBLE first: commander leads with `Usage:` and puts the
    // description after it; clap leads with the description on every
    // subcommand. `help_template` is not inherited by subcommands in clap
    // derive, so doing this on the rendered text covers all ~50 command paths
    // with one mechanism instead of an attribute on every struct.
    let help = &normalise_preamble(help);
    let lines: Vec<&str> = help.lines().collect();
    let first = lines.iter().position(|l| is_header(l));
    let Some(first) = first else {
        return help.to_string();
    };
    // `Usage:` is part of the preamble, not a movable section.
    let mut preamble: Vec<&str> = lines[..first].to_vec();
    let mut sections: Vec<(String, Vec<&str>)> = Vec::new();
    let mut cur: Option<(String, Vec<&str>)> = None;
    for line in &lines[first..] {
        if is_header(line) {
            if let Some(sec) = cur.take() {
                sections.push(sec);
            }
            cur = Some(((*line).to_string(), Vec::new()));
        } else if let Some((_, body)) = cur.as_mut() {
            body.push(line);
        }
    }
    if let Some(sec) = cur.take() {
        sections.push(sec);
    }
    // `Usage:` renders as a header but belongs with the preamble.
    while sections
        .first()
        .is_some_and(|(h, _)| h.starts_with("Usage:"))
    {
        let (h, body) = sections.remove(0);
        preamble.push(Box::leak(h.into_boxed_str()));
        preamble.extend(body);
    }

    let rank = |h: &str| match h {
        _ if h.starts_with("Arguments:") => 0,
        _ if h.starts_with("Options:") => 1,
        _ if h.starts_with("Commands:") => 2,
        _ => 3,
    };
    sections.sort_by_key(|(h, _)| rank(h));

    let mut out = preamble.join("\n");
    for (h, body) in sections {
        // Exactly one blank line before every section header, as commander
        // renders it. Rebuilding the blocks drops whatever spacing they had.
        while out.ends_with('\n') {
            out.pop();
        }
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&h);
        out.push('\n');
        out.push_str(&body.join("\n"));
        out.push('\n');
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn commander_help(e: &clap::Error) -> String {
    let mut help = e.to_string();
    for (canonical, alias) in [
        ("--allowedTools", "--allowed-tools"),
        ("--disallowedTools", "--disallowed-tools"),
        ("--bg", "--background"),
    ] {
        let heading = format!("{canonical}, {alias}");
        help = help.replacen(canonical, &heading, 1);
        let alias_paragraph = format!(
            "\n          \n          [aliases: {}]",
            alias.trim_start_matches("--")
        );
        help = help.replace(&alias_paragraph, "");
    }
    reorder_help_sections(&help)
}

fn command_runs_config_startup(command: Option<&crate::commands::Commands>) -> bool {
    match command {
        Some(crate::commands::Commands::Project(project)) => project.command.is_some(),
        Some(
            crate::commands::Commands::Sandbox(_)
            | crate::commands::Commands::Attach(_)
            | crate::commands::Commands::RemoteControl(_)
            | crate::commands::Commands::Rm(_)
            | crate::commands::Commands::Daemon(_)
            | crate::commands::Commands::BgRun(_)
            | crate::commands::Commands::BgPtySession(_),
        ) => false,
        _ => true,
    }
}

fn command_initializes_user_id(command: Option<&crate::commands::Commands>) -> bool {
    matches!(
        command,
        Some(
            crate::commands::Commands::Mcp(_)
                | crate::commands::Commands::Doctor(_)
                | crate::commands::Commands::SetupToken(_)
                | crate::commands::Commands::Install(_)
                | crate::commands::Commands::Update(_)
        )
    )
}

/// Materialize first-run configuration and run the versioned startup
/// migrations at Claude's pre-command boundary. Specialized fast paths that
/// bypass this block in the oracle are filtered by
/// [`command_runs_config_startup`].
async fn run_config_startup(command: Option<&crate::commands::Commands>) {
    if !command_runs_config_startup(command) {
        return;
    }
    let (Some(global_config_path), Some(lingxi_home)) = (
        migrations::global_config::global_config_path(),
        migrations::global_config::lingxi_config_home(),
    ) else {
        return;
    };

    let migration_pending = migrations::global_config::read_map(&global_config_path)
        .map(|map| {
            map.get("migrationVersion")
                .and_then(serde_json::Value::as_u64)
                != Some(migrations::CURRENT_MIGRATION_VERSION)
        })
        .unwrap_or(false);
    let initializes_device_identity = command.is_none() || command_initializes_user_id(command);

    if migration_pending || initializes_device_identity {
        let first_start_time =
            chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        if let Err(error) = migrations::global_config::ensure_first_start_metadata(
            &global_config_path,
            &first_start_time,
            traits::CLAUDE_CODE_VERSION,
        ) {
            tracing::warn!(%error, "first-start metadata write failed");
        }
    }
    if migration_pending || initializes_device_identity {
        let _ = migrations::global_config::ensure_machine_id(&global_config_path);
    }

    let project_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let env = migrations::MigrationEnv {
        global_config_path,
        lingxi_config_home: lingxi_home,
        project_dir,
        ctx: migrations::MigrationContext::from_env(),
        bus: None,
    };
    // `Lm(e)` in 2.1.245 runs this unversioned migration immediately before
    // the version-13 set; it is intentionally no longer a runner member.
    migrations::migrate_mcp_servers::run(&env).await;
    migrations::run_migrations(&env).await;

    // These command families reach the device identity during their own
    // startup in Claude 2.1.245; keep it after the migration block so the
    // resulting top-level key order matches the oracle.
    if command_initializes_user_id(command) {
        let _ = migrations::global_config::get_or_create_user_id();
    }

    // Async fire-and-forget (TS `.catch(() => {})`): retried next startup.
    tokio::spawn(async move {
        migrations::migrate_changelog_from_config(&env).await;
    });
}

/// Top-level entrypoint. Returns the process exit code.
pub async fn run_cli(args: Vec<OsString>) -> i32 {
    if let Some(code) = commands::plugin_eval_mock::run_from_env().await {
        return code;
    }
    startup_trace::start();
    // Freeze the startup environment BEFORE anything can apply a settings-file
    // `env` to the process. `${VAR}` inside a MANAGED MCP allow/deny matcher
    // expands against this snapshot, so taking it late would let a lower-trust
    // settings tier steer what an enterprise policy matches. The oracle gets
    // this ordering implicitly (`Dut()` calls `NQr()` first); here it is
    // explicit, and this is the earliest point in the process.
    mcp::enterprise_policy::prime_startup_env();
    let mut parsed = match Argv::from_iter(args.clone()) {
        Ok(a) => a,
        Err(e) => {
            // claude-code (commander) renders several argv errors differently
            // from clap — a single/two-line message with no "Usage:"/"For more
            // information" block. Reformat the ones we can match byte-for-byte
            // (missing-arg, unknown-option, unknown-command + suggestion);
            // everything else keeps clap's rendering (the remaining
            // clap-vs-commander help-block layout difference).
            if let Some(msg) = commander_error(&e, &args) {
                eprintln!("{msg}");
                return exit_codes::ARGV_ERROR;
            }
            // Help is normalized to commander's visible-alias presentation;
            // other clap errors/version output retain clap's renderer.
            if e.kind() == ErrorKind::DisplayHelp {
                print!("{}", commander_help(&e));
            } else {
                e.print().ok();
            }
            return match e.kind() {
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => exit_codes::SUCCESS,
                _ => exit_codes::ARGV_ERROR,
            };
        }
    };
    startup_trace::init(parsed.debug_filter());
    startup_trace::mark("argv_parse");

    if parsed.restricted_enabled() {
        std::env::set_var("LINGXI_RESTRICTED", "1");
    }

    // `--debug` is now `Option<String>` (optional category filter); collapse to
    // on/off for logging init. `--mcp-debug` (deprecated alias) and `--debug-file`
    // also imply debug mode.
    // The interactive fullscreen TUI owns the terminal — suppress stderr logging
    // there so WARN lines don't corrupt the rendered frame. A subcommand
    // (mcp/auth/…) or print/stdio mode keeps the normal stderr logger.
    let interactive_tui = matches!(
        parsed.command.as_ref(),
        Some(crate::commands::Commands::BgPtySession(_))
    ) || (parsed.command.is_none()
        && matches!(crate::mode::decide_mode(&parsed), crate::mode::Mode::Tui));
    startup_trace::mark("mode_decide_for_logging");
    logging::init(parsed.debug_enabled(), interactive_tui);
    let telemetry_records_session = parsed.command.is_none()
        || matches!(
            parsed.command.as_ref(),
            Some(crate::commands::Commands::BgPtySession(_))
        );
    let managed_otel_overrides = engine_desktop::managed_otel_env_overrides().await;
    let _telemetry_guard =
        match telemetry::otel::OtelConfig::from_env_with_managed(&managed_otel_overrides) {
            cfg if cfg.enabled => telemetry::otel::install_process_with_config(
                "lingxi-cli",
                telemetry_records_session,
                cfg,
            ),
            _ => telemetry::otel::TelemetryGuard::disabled(),
        };
    tracing::debug!(?parsed, "argv parsed");

    // (M3 cc2.1.198) `--bg`/`--background` × `--print`/`-p` is rejected UP
    // FRONT — before any mode/subcommand dispatch — matching the binary's bg
    // fast path (`handleBgFlag` runs from the top-level dispatcher on any
    // `--bg`/`--background` token and its `pof` validator rejects before a
    // session dir is even minted). Bare message + `\n` on stderr (no `Error:`
    // prefix), exit 1 (`process.exitCode=1`).
    if let Err(msg) = parsed.validate_background_args() {
        eprintln!("{msg}");
        return exit_codes::ARGV_ERROR;
    }

    // Claude Code 2.1.246 validates the entire `--agents` record before any
    // runtime/auth work. Safe mode deliberately ignores the flag without
    // parsing it; bare mode still validates it.
    let safe_mode = parsed.safe_mode
        || traits::env::is_env_truthy(std::env::var("LINGXI_SAFE_MODE").ok().as_deref());
    if !safe_mode {
        if let Some(raw) = parsed.agents.as_deref() {
            if let Err(error) = agent::parse_agents_from_flag_json_checked(raw) {
                eprintln!("Error: Invalid --agents configuration:\n{error}");
                return exit_codes::ARGV_ERROR;
            }
        }
    }

    // (M4 cc2.1.198) `--effort` argParser warning. The binary validates INSIDE
    // commander's argParser (`u4i` — so the warning prints during argv parse,
    // before any dispatch) and continues with the flag ignored:
    // `process.stderr.write(`Warning: ${l}\n`)`. Verified live: stderr
    // `Warning: Unknown --effort value 'banana' — ignoring it and using the
    // default effort. Valid values: low, medium, high, xhigh, max.`, run
    // continues. The normalized level threads via `resolve_desktop_config`.
    if let (_, Some(warning)) = parsed.normalized_effort() {
        eprintln!("{warning}");
    }

    // (M3 cc2.1.198) `--bare` exports `LINGXI_SIMPLE=1` for this process +
    // children (binary top dispatcher @224048363: `if((l===-1?t:t.slice(0,l))
    // .includes("--bare"))process.env.CLAUDE_CODE_SIMPLE="1"` — pre-`--`
    // tokens only, which clap's parse already honors). `xd()` also treats a
    // pre-set truthy env as bare, mirrored in `resolve_desktop_config`.
    if parsed.bare {
        std::env::set_var("LINGXI_SIMPLE", "1");
    }
    // (M3 cc2.1.198) safe mode (`Ql()` = `--safe-mode` OR truthy env) exports
    // `LINGXI_SAFE_MODE=1` + `LINGXI_DISABLE_LINGXI_MDS=1` (binary @223917313:
    // `if(Ql())process.env.CLAUDE_CODE_SAFE_MODE="1",process.env.
    // CLAUDE_CODE_DISABLE_CLAUDE_MDS="1"`); the latter is the orchestrator's
    // existing LINGXI.md kill-switch (`orchestrator::prompt::memory_block`),
    // so subagents/children inherit the disable too.
    if parsed.safe_mode
        || traits::env::is_env_truthy(std::env::var("LINGXI_SAFE_MODE").ok().as_deref())
    {
        std::env::set_var("LINGXI_SAFE_MODE", "1");
        std::env::set_var("LINGXI_DISABLE_LINGXI_MDS", "1");
    }

    // (M-01, cc2.1.215) `--brief` exports `LINGXI_BRIEF=1` for this process +
    // children (CC registry key `CLAUDE_CODE_BRIEF`; both honored by the tool
    // gate). Mirrors CC's `CAn(e)`, where `e.brief` and `Z.CLAUDE_CODE_BRIEF`
    // are equivalent triggers and the env is what the `SendUserMessage` tool's
    // `isBriefEnabled`/`aKr()` gate reads. Default-off: without this flag the
    // Brief tool stays invisible to the model (see `tool_ui::brief`).
    if parsed.brief {
        std::env::set_var("LINGXI_BRIEF", "1");
    }

    // (CLI-12, cc2.1.238) `--messaging-socket-path <path>` (@307414302) pins
    // the cross-session messaging socket instead of the auto-generated path.
    // Recorded here, before ANY code path can bind the inbox
    // (`mode::ensure_live_messaging` is the only binder and runs far later, in
    // the TUI/session mounts).
    if let Some(path) = parsed.messaging_socket_path.as_deref() {
        crate::mode::set_messaging_socket_override(path);
    }

    // (CLI-15) `--append-subagent-system-prompt` carries the oracle's implication
    // `wby(e,t=process.env){if(e)t.CLAUDE_CODE_ENABLE_APPEND_SUBAGENT_PROMPT="1"}`
    // (@306637528) — the flag turns its own gate on.
    //
    // The SPLICE half is now wired too: the value rides
    // `LINGXI_APPEND_SUBAGENT_SYSTEM_PROMPT` and
    // `agent::handle::append_subagent_system_prompt_suffix` folds it onto every
    // subagent's rendered system prompt as the final section, gated on the env
    // flag above (oracle @292360822:
    // `Zt=!C&&!d?.isolatedContext&&Un(process.env.CLAUDE_CODE_ENABLE_APPEND_SUBAGENT_PROMPT)
    //     &&r.options.appendSubagentSystemPrompt?Rm([...Xt,r.options.appendSubagentSystemPrompt]):Xt`).
    //
    // The env pair is the transport because the subagent spawner is reached
    // through `engine-desktop`'s runtime build, which carries no per-spawn CLI
    // options channel; it is also what gives the oracle's "propagated to nested
    // subagents" for free — a nested spawn is a child of the same process and
    // reads the same variables. Both are set BEFORE any runtime is constructed.
    if let Some(text) = parsed.append_subagent_system_prompt.as_deref() {
        std::env::set_var("LINGXI_ENABLE_APPEND_SUBAGENT_PROMPT", "1");
        std::env::set_var("LINGXI_APPEND_SUBAGENT_SYSTEM_PROMPT", text);
    }

    // (CLI-01, cc2.1.238) `--autocompact <auto|tokens>` projects onto
    // `LINGXI_AUTO_COMPACT_WINDOW`, the port's only auto-compact-window pin
    // (`compaction::thresholds::effective_context_window_size` clamps the model
    // context window with it, and `/autocompact` reports it as
    // `WindowSource::Env`). The oracle resolves the flag with
    // `lvp(t.autocompact, Vo().autoCompactWindow)`: `auto` yields `undefined`
    // and therefore DROPS the configured window, so an explicit `auto` clears a
    // pre-set env pin here rather than merely leaving it alone.
    match parsed.autocompact {
        Some(argv::AutocompactWindow::Auto) => std::env::remove_var("LINGXI_AUTO_COMPACT_WINDOW"),
        Some(argv::AutocompactWindow::Tokens(tokens)) => {
            std::env::set_var("LINGXI_AUTO_COMPACT_WINDOW", tokens.to_string());
        }
        None => {}
    }

    // Custom beta headers are an API-key-only Anthropic surface. Validate at
    // startup so OAuth/non-key sessions do not appear to accept an inert flag.
    if let Some(raw_betas) = parsed.betas.as_mut() {
        let mut seen = std::collections::HashSet::new();
        raw_betas.retain(|beta| {
            let trimmed = beta.trim();
            !trimmed.is_empty() && seen.insert(trimmed.to_string())
        });
        for beta in raw_betas.iter_mut() {
            *beta = beta.trim().to_string();
            if beta.contains(',') || beta.chars().any(char::is_control) {
                eprintln!("lingxi-cli: invalid --betas value `{beta}`");
                return exit_codes::ARGV_ERROR;
            }
        }
        if !raw_betas.is_empty() {
            if std::env::var("ANTHROPIC_API_KEY")
                .ok()
                .is_none_or(|key| key.trim().is_empty())
            {
                eprintln!("lingxi-cli: --betas is only available for Anthropic API-key users.");
                return exit_codes::RUNTIME_ERROR;
            }
            if parsed.model.as_deref().is_some_and(|model| {
                let (profile, _) = llm_client::split_profile_model(model);
                profile != "anthropic"
            }) {
                eprintln!("lingxi-cli: --betas is only supported by the Anthropic provider.");
                return exit_codes::RUNTIME_ERROR;
            }
        }
    }

    // `--cwd <dir>` must apply BEFORE the subcommand dispatch, not just for
    // session modes: the subcommands resolve their target project from the LIVE
    // process cwd (mcp via `current_dir()` → project key + `<cwd>/.mcp.json`,
    // doctor, and — destructively — `project purge`, whose default target is the
    // current dir). If we chdir only after dispatch, `--cwd /other mcp add …`
    // writes the WRONG project's config and `--cwd /other project purge` deletes
    // the WRONG project's transcripts. `apply_cwd` only validates + `set_current_dir`
    // (no other side effects), so it is safe to run first for all modes.
    if let Err(e) = cwd::apply_cwd(parsed.cwd.as_deref()) {
        eprintln!("lingxi-cli: {e}");
        return exit_codes::RUNTIME_ERROR;
    }

    // These public flags depend on Anthropic-private services/protocols. Never
    // accept them as inert success and never route them to LingXi's unrelated
    // local desktop bridge.
    if parsed.no_chrome {
        // This flag is an explicit negative capability, not an accepted no-op.
        // The setting is inherited by background/session children and is the
        // single gate future optional browser integrations must consult.
        std::env::set_var("LINGXI_DISABLE_CHROME", "1");
    }
    if parsed.chrome {
        eprintln!(
            "lingxi-cli: --chrome requires the unavailable Anthropic Chrome extension protocol."
        );
        return exit_codes::NOT_IMPLEMENTED;
    }
    if parsed.restricted_enabled() && parsed.remote_control.is_some() {
        eprintln!("Cloud sessions cannot be created from a --restricted session");
        return exit_codes::RUNTIME_ERROR;
    }
    if parsed.remote_control.is_some() {
        eprintln!(
            "lingxi-cli: --remote-control requires the unavailable Anthropic relay/auth protocol."
        );
        return exit_codes::NOT_IMPLEMENTED;
    }

    // Managed enterprise startup version gate (parity 2.1.207 H-BIN-09,
    // CC `c1p`/`a1p` on the fast startup path). If a managed (`policySettings`)
    // tier pins `requiredMinimumVersion`/`requiredMaximumVersion` and this
    // binary's version is outside the org-approved range, print the byte-exact
    // instruction message to stderr and exit 1 — BEFORE any subcommand dispatch
    // or session build. `update`/`install`/`doctor` are exempt (a pinned-out
    // user must be able to fix their install); a bare session (no command) is
    // gated. Reads the OS-level managed dir (cwd-independent, already-applied
    // `--cwd` is irrelevant). Fail-open on an unreadable/malformed policy.
    {
        let top_level = parsed
            .command
            .as_ref()
            .map(crate::commands::Commands::top_level_name);
        let managed_tiers = engine_desktop::settings_watch::managed_settings_raw_tiers().await;
        let policy = engine::settings::enterprise::managed_version_policy(&managed_tiers);
        if let Some(msg) = engine::settings::enterprise::version_gate(
            env!("CARGO_PKG_VERSION"),
            policy.required_minimum_version.as_deref(),
            policy.required_maximum_version.as_deref(),
            top_level,
            &mut |m| tracing::error!("{m}"),
        ) {
            eprintln!("{msg}");
            return exit_codes::RUNTIME_ERROR;
        }
    }

    // Freeze `processWrapper` before any command can spawn the daemon, a
    // background worker, or another copy of this CLI. The resolver deliberately
    // excludes project/local settings; descendants inherit this immutable argv
    // snapshot instead of re-reading a possibly different working directory.
    let wrapper_cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(error) => {
            eprintln!("lingxi-cli: failed to resolve cwd for processWrapper: {error}");
            return exit_codes::RUNTIME_ERROR;
        }
    };
    if let Err(error) = process_wrapper::configure(parsed.settings.as_deref(), &wrapper_cwd).await {
        eprintln!("lingxi-cli: {error}");
        return exit_codes::RUNTIME_ERROR;
    }
    if let Some(settings) = crate::init::parse_flag_settings(parsed.settings.as_deref()) {
        if let Ok(value) = serde_json::to_value(settings) {
            let _ = mcp::enterprise_policy::install_flag_settings_policy(value);
        }
    }

    run_config_startup(parsed.command.as_ref()).await;

    // Top-level subcommand dispatch (mcp/auth/plugin/project/setup-token/agents/
    // install/update/doctor/auto-mode/ultrareview). When clap matched a leading
    // command token, run that family and exit — this is what stops a bare `mcp`/
    // `auth` token from being swallowed as a billable chat prompt.
    if let Some(command) = parsed.command.clone() {
        return command.run().await;
    }

    // Session-only network resources are resolved before any orchestrator/TUI
    // state is constructed. Background sessions defer this into the PTY child:
    // resolving a plugin URL here would serialize a path below this process's
    // temporary guard and delete it as soon as daemonization returned.
    let _startup_resources = if parsed.background {
        None
    } else {
        match startup_resources::prepare(&mut parsed).await {
            Ok(guard) => Some(guard),
            Err(e) => {
                eprintln!("lingxi-cli: {e}");
                return exit_codes::RUNTIME_ERROR;
            }
        }
    };

    // `--session-id <uuid>` validation (claude-code main.tsx:1276-1300), byte-exact
    // messages + exit 1. Runs for every session mode (print/TUI/REPL) before any
    // runtime is built; the validated id then threads into
    // `DesktopConfig.session_id_override` via `resolve_desktop_config`.
    if let Some(sid) = parsed.session_id.as_deref() {
        // (a) cross-flag rule: pairing --session-id with --continue/--resume
        //     requires --fork-session (else the resumed session's own id wins).
        if (parsed.continue_session || parsed.resume.is_some()) && !parsed.fork_session {
            eprintln!(
                "Error: --session-id can only be used with --continue or --resume if --fork-session is also specified."
            );
            return exit_codes::ARGV_ERROR;
        }
        // (b) UUID validation (a bare UUID; `parse_prefixed` also tolerates the
        //     `sess:`-prefixed display form).
        if protocol::SessionId::parse_prefixed(sid).is_none() {
            eprintln!("Error: Invalid session ID. Must be a valid UUID.");
            return exit_codes::ARGV_ERROR;
        }
        // (c) A user-supplied fresh session id must not reuse any existing
        //     transcript path, even from another project under the same config
        //     home. Reject before runtime construction so no append/write path
        //     is opened against the occupied JSONL.
        if let Some(parsed_id) = protocol::SessionId::parse_prefixed(sid) {
            match session_id_exists_in_store(&run::lingxi_home_dir(), parsed_id.as_uuid()).await {
                Ok(true) => {
                    eprintln!("Error: Session ID {sid} is already in use.");
                    return exit_codes::ARGV_ERROR;
                }
                Ok(false) => {}
                Err(error) => {
                    eprintln!("Error: Unable to verify session ID uniqueness: {error}");
                    return exit_codes::RUNTIME_ERROR;
                }
            }
        }
    }

    // `--setting-sources <user,project,local>` token validation (claude-code
    // main.tsx): each comma-split token must be one of user/project/local
    // (CASE-SENSITIVE, raw token echoed) else hard-error with the byte-exact
    // message + exit 1. An empty string is VALID (treated as no sources).
    if let Some(sources) = parsed.setting_sources.as_deref() {
        for tok in sources.split(',').map(str::trim).filter(|t| !t.is_empty()) {
            if !matches!(tok, "user" | "project" | "local") {
                eprintln!(
                    "Error processing --setting-sources: Invalid setting source: {tok}. Valid options are: user, project, local"
                );
                return exit_codes::ARGV_ERROR;
            }
        }
    }

    // (`cwd::apply_cwd` already ran above, before the subcommand dispatch.)

    // (Item B) Resolve the session permission mode from CLI flags + settings,
    // run the bypass safety guards, and capture the startup notice. This must
    // happen AFTER `cwd::apply_cwd` (so the project `.lingxi/settings.json` is
    // read from the effective project dir) and BEFORE `build_runtime` — a
    // refused bypass exits before the runtime is constructed, and the resolved
    // mode threads into `DesktopConfig.permission_mode`.
    //
    // (Task 8) The resolution itself is now the shared `resolve_permission_mode`
    // helper so the interactive TUI/REPL paths resolve the SAME mode without
    // re-implementing it. The bypass-safety GUARD stays HERE in `run_cli`: it
    // runs exactly once, before mode dispatch, for ALL modes — a refusal exits 1
    // before any runtime is built, so the interactive paths never re-run it.
    let (permission_mode, permission_notice) = resolve_permission_mode(&parsed);
    if parsed.restricted_enabled()
        && (permission_mode == permission::PermissionMode::BypassPermissions
            || parsed.dangerously_skip_permissions
            || parsed.allow_dangerously_skip_permissions)
    {
        eprintln!("bypassPermissions not supported in restricted mode");
        return exit_codes::RUNTIME_ERROR;
    }
    // Guards run when bypass is requested OR resolved (setup.ts:396).
    if permission_mode == permission::PermissionMode::BypassPermissions
        || parsed.dangerously_skip_permissions
    {
        if let Err(msg) = permission::enforce_bypass_safety(&bypass_env::RealBypassEnv::new()).await
        {
            eprintln!("{msg}");
            return exit_codes::RUNTIME_ERROR; // TS process.exit(1)
        }
    }

    // Background dispatch happens only after session/settings validation and
    // permission safety enforcement. The dispatcher then runs the foreground
    // trust/bypass setup UI and records that approval in launch.json before it
    // daemonises; the hidden PTY child never owns unattended setup dialogs.
    if parsed.background {
        return crate::background_dispatch::dispatch_background(&parsed, permission_mode).await;
    }

    // (CLI-13/16 + SC-09, cc2.1.238) The truncating-resume / rewind cross-flag
    // gates. In the oracle these are the FIRST four statements of `runHeadless`
    // after `after_grove_check` (@307217538), ahead of every other headless
    // gate, and each writes its line to stderr and exits 1:
    //
    // ```js
    // if(c.resumeSessionAt&&!c.resume){…"Error: --resume-session-at requires --resume"}
    // if(c.resumeDropsTurn!==void 0&&!c.resumeSessionAt){…"Error: --resume-drops-turn requires --resume-session-at"}
    // if(c.rewindFiles&&!c.resume){…"Error: --rewind-files requires --resume"}
    // if(c.rewindFiles&&t){…"Error: --rewind-files is a standalone operation and cannot be used with a prompt"}
    // ```
    //
    // They are therefore print-mode-only, exactly as both flags' help text says
    // ("Ignored outside print mode"); `validate_truncating_resume_args` carries
    // that condition.
    if let Err(msg) = parsed.validate_truncating_resume_args() {
        eprintln!("Error: {msg}");
        return exit_codes::ARGV_ERROR;
    }

    // (CLI-13) The truncating resume itself lives in
    // `crate::resume_truncation::apply_truncating_resume`, applied by
    // `run::resume_resolved_session` immediately after the transcript load —
    // the oracle's own position (@307370121). It stops the chain at the named
    // entry and, when `--resume-drops-turn` is supplied, REFUSES when the
    // discarded range holds anything not attributable to the declared turn
    // (the `AEy` classifier, @306799802). This module used to refuse the flag
    // outright; that refusal is gone now that the behaviour exists.

    // P3 cross-flag validation for --input-format=stream-json and
    // --replay-user-messages (§4.1 SPEC-inferred.md, exact error strings).
    if let Err(msg) = parsed.validate_stream_json_input_args() {
        eprintln!("Error: {msg}");
        return exit_codes::ARGV_ERROR;
    }

    // (M4 cc2.1.198) A truthy `--prompt-suggestions` requires --print +
    // --output-format=stream-json. Binary order: this `Es(...)` gate runs
    // IMMEDIATELY BEFORE the include-partial-messages gate (same statement
    // chain in the main action). Byte-exact message + exit 1 (verified live).
    if let Err(msg) = parsed.validate_prompt_suggestions_args() {
        eprintln!("Error: {msg}");
        return exit_codes::ARGV_ERROR;
    }

    // `--include-partial-messages` requires BOTH --print and
    // --output-format=stream-json (claude-code main.tsx:1848-1852). Byte-exact
    // error + exit 1 (ARGV_ERROR). Without this gate lingxi silently accepted
    // the misuse and ran a (billable) turn.
    if parsed.include_partial_messages && !(parsed.print && parsed.is_stream_json()) {
        eprintln!(
            "Error: --include-partial-messages requires --print and --output-format=stream-json."
        );
        return exit_codes::ARGV_ERROR;
    }

    // (2.1.211) `--forward-subagent-text` (binary `xe = k ||
    // CLAUDE_CODE_FORWARD_SUBAGENT_TEXT`) forwards subagent text/thinking blocks
    // as assistant/user messages with a non-null `parent_tool_use_id`. Requires
    // BOTH --print and --output-format=stream-json. The binary places this gate
    // immediately AFTER the include-partial-messages gate: an EXPLICIT flag in
    // the wrong context is a fatal error, while an env-only opt-in silently
    // disables. Byte-exact message + exit 1.
    if parsed.forward_subagent_text && !(parsed.print && parsed.is_stream_json()) {
        eprintln!(
            "Error: --forward-subagent-text requires --print and --output-format=stream-json."
        );
        return exit_codes::ARGV_ERROR;
    }

    // (M3 cc2.1.198) `--no-session-persistence` requires `--print`. Binary
    // order: this gate runs immediately AFTER the include-partial-messages
    // gate (@223929381, next statement). Byte-exact message + exit 1.
    if let Err(msg) = parsed.validate_session_persistence_args() {
        eprintln!("Error: {msg}");
        return exit_codes::ARGV_ERROR;
    }

    // (C5) `--plan-mode-instructions` is `--print`-only; same surfacing so the
    // final stderr is byte-exact `Error: --plan-mode-instructions can only be
    // used with --print mode.`
    if let Err(msg) = parsed.validate_plan_mode_instructions_args() {
        eprintln!("Error: {msg}");
        return exit_codes::ARGV_ERROR;
    }

    // (CLI-16, cc2.1.238) `--rewind-files <user-message-id>` — "Restore files to
    // state at the specified user message and exit (requires --resume)". In the
    // oracle this branch sits inside `runHeadless` right after the transcript
    // loads (@307222364) and ALWAYS exits, so it never reaches the prompt gate
    // or the `--output-format=stream-json requires --verbose` gate below it —
    // hence its position HERE, ahead of both output-format branches, rather
    // than down with the `--continue`/`--resume` dispatch. Its two argv gates
    // fired above; the sink is built inline because `make_sink` is defined
    // after the `&mut parsed` startup-resource pass.
    if parsed.print && parsed.rewind_files.is_some() {
        let sink: Arc<dyn output::OutputSink> = if parsed.is_json_output() {
            Arc::new(output::JsonSink::new(protocol::SessionId::new()))
        } else {
            Arc::new(output::PlainSink::new())
        };
        return run::run_rewind_files(&parsed, sink.as_ref()).await;
    }

    // stream-json: `--output-format stream-json --verbose` (print-only, no
    // SinkAdapter/OutputSink layer — the StreamJsonStream IS the OutputStream).
    //
    // Gate: when `--print`/`-p` is combined with `stream-json` the caller MUST
    // also pass `--verbose` (mirrors claude-code's argv validation:
    // `main.tsx printMode && !verbose && outputFormat=="stream-json"` →
    // "When using --print, --output-format=stream-json requires --verbose").
    if parsed.is_stream_json() {
        if parsed.print && !parsed.verbose {
            eprintln!("Error: When using --print, --output-format=stream-json requires --verbose");
            return exit_codes::ARGV_ERROR;
        }
        let stream = Arc::new(stream_json::StreamJsonStream::new_placeholder());
        traits::OutputStream::set_thinking_display(
            stream.as_ref(),
            parsed.thinking_display.as_deref(),
        );
        // P4: wire --include-partial-messages and --include-hook-events flags
        // before build_runtime so the stream is fully configured before any
        // hook or SSE events flow through it.
        stream.set_flags(parsed.include_partial_messages, parsed.include_hook_events);
        // (2.1.211) Carry the effective `--forward-subagent-text` state (flag OR
        // truthy CLAUDE_CODE_FORWARD_SUBAGENT_TEXT), gated to the valid --print +
        // stream-json context — an explicit flag in the wrong context already
        // errored above; an env-only opt-in in the wrong context stays disabled.
        stream.set_forward_subagent_text(
            parsed.forward_subagent_text_effective() && parsed.print && parsed.is_stream_json(),
        );
        let adapter: Arc<dyn traits::OutputStream> = stream.clone();

        // P5 Phase 2: for the bidirectional `--input-format stream-json` path,
        // build the shared control plane BEFORE `build_runtime` (its outbound
        // handle comes from the stream, available now) and inject the
        // `can_use_tool` permission decider as the inner transport. The
        // `PolicyPermissionGate` (enforcement default on) wraps it as the OUTER
        // local pre-check, so only an unresolved `Ask` round-trips over stdio.
        // The output-only print path keeps the headless deny-on-ask default
        // (no stdin reader to answer a control_response).
        let control_plane = if parsed.is_stream_json_input() {
            let plane = control_plane::StdioControlPlane::new(stream.outbound_tx());
            // GATE-SYSMSG-01: share the stream's session-id handle so a locally
            // denied tool emits a `permission_denied` system message stamped with
            // the same `session_id` as every data frame.
            plane.set_session_id(stream.session_id_handle());
            Some(plane)
        } else {
            None
        };

        let runtime = if let Some(plane) = &control_plane {
            let mut cfg = init::resolve_desktop_config(&parsed, permission_mode);
            // §2b: persist an ALLOW response's `updatedPermissions` rule updates to
            // the SAME settings tree the engine resolved (claude-code
            // `persistPermissionUpdates`). Paths come from the same `cfg` so a
            // host-allowed rule lands where the next session loads it.
            let gate = Arc::new(
                control_plane::StdioControlPermissionGate::new(plane.clone()).with_persist(
                    permission::PermissionPaths {
                        lingxi_home: cfg.lingxi_home.clone(),
                        cwd: cfg.cwd.clone(),
                    },
                ),
            );
            // SH-02: keep a typed handle so the `permission_prompt` notifier
            // can be attached once the orchestrator exists (the gate itself has
            // to be built FIRST — it is injected into the runtime config).
            let gate_handle = gate.clone();
            cfg.injected_permission_gate = Some(gate as Arc<dyn permission::gate::PermissionGate>);
            let rt = match init::build_runtime_from_config(cfg, adapter).await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("lingxi-cli: {e}");
                    return exit_codes::RUNTIME_ERROR;
                }
            };
            // SH-02 — this is what makes `Cou` reachable: without it the gate's
            // `OnceLock` stays empty and every armed guard is inert.
            gate_handle.set_prompt_notifier(Arc::new(
                permission_prompt_notify::OrchestratorPermissionPromptNotifier::new(
                    rt.orchestrator.clone(),
                ),
            ));
            rt
        } else {
            match init::build_runtime(&parsed, adapter, permission_mode).await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("lingxi-cli: {e}");
                    return exit_codes::RUNTIME_ERROR;
                }
            }
        };
        if let Some(notice) = startup_deprecation_notice(&parsed) {
            eprintln!("{notice}");
        }
        if let Some(notice) = &permission_notice {
            eprintln!("{notice}");
        }
        // P3: when --input-format=stream-json is also set, use the multi-turn
        // stdin loop instead of the single-prompt one-shot path.
        if let Some(plane) = control_plane {
            return run::run_stream_json_input_loop(
                &parsed,
                &runtime,
                stream,
                permission_mode,
                plane,
            )
            .await;
        }
        return run::run_stream_json_print(&parsed, &runtime, stream, permission_mode).await;
    }

    // `--output-format json` / `--json` in PRINT mode: emit only the final
    // result JSON line (suppress all streaming frames). Only intercept when
    // a non-slash prompt is present or `--print` is active (no slash prompt)
    // — slash commands keep the old JSON event format, interactive/REPL mode
    // with `--json` falls through to the normal dispatch path (repl.rs handles it).
    let is_non_slash_print = parsed
        .prompt
        .as_deref()
        .map_or(false, |p| !p.trim_start().starts_with('/'));
    if parsed.is_json_output() && (is_non_slash_print || parsed.print) {
        let stream = Arc::new(stream_json::StreamJsonStream::new_json_mode_placeholder());
        let adapter: Arc<dyn traits::OutputStream> = stream.clone();
        let runtime = match init::build_runtime(&parsed, adapter, permission_mode).await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("lingxi-cli: {e}");
                return exit_codes::RUNTIME_ERROR;
            }
        };
        if let Some(notice) = startup_deprecation_notice(&parsed) {
            eprintln!("{notice}");
        }
        if let Some(notice) = &permission_notice {
            eprintln!("{notice}");
        }
        return run::run_json_print(&parsed, &runtime, stream, permission_mode).await;
    }

    let chosen = mode::decide_mode(&parsed);
    startup_trace::mark("mode_decide");

    // (2.1.201) Resolve + cache the accessibility screen-reader gate ONCE. This
    // is independent of the engine runtime, so keep it before the mode-specific
    // runtime build instead of forcing fresh TUI/REPL paths to pre-build and
    // discard a generic runtime.
    let ax_config = init::load_settings_ax_screen_reader(&parsed);
    ax_screen_reader::init(parsed.ax_screen_reader, ax_config);
    // `if(!L && process.stdout.isTTY && IO()) console.log("[Accessible screen
    // reader mode: on]")` — `L` == print mode. Emitted for interactive stdout.
    {
        use std::io::IsTerminal as _;
        ax_screen_reader::maybe_announce(parsed.print, std::io::stdout().is_terminal());
    }

    // (W40-follow-up) One-time startup notices, the bounded stand-in for
    // claude-code's startup notification queue (`main.tsx:2872-2896`). TS pushes
    // up to two HIGH-priority notices onto a UI queue rendered above the REPL:
    // the model-deprecation warning and the permission-mode notice. The Rust CLI
    // has no UI notification queue, so — per the brief — we emit the bounded
    // subset as a plain startup line instead of building that abstraction.
    //
    // Surfaced here: the DEPRECATION warning. `startup_deprecation_notice`
    // resolves the same initial model `resolve_desktop_config` threads into the
    // engine (`--model`, else the desktop default — the analog of TS
    // `resolvedInitialModel = parseUserSpecifiedModel(initialMainLoopModel ??
    // getDefaultMainLoopModel())`) and returns `Some` only when that model is
    // deprecated for the active provider. It goes to STDERR so `--json` stdout
    // stays parseable (the same channel discipline the REPL's `"> "` prompt
    // uses). SAFETY: with any current (Claude 4-generation) default model the
    // lookup is `None`, so this prints nothing and startup is byte-identical.
    //
    // The sibling permission-mode notice (TS `permissionModeNotification`,
    // `main.tsx:2882-2887`) is now wired below: `initialPermissionModeFromCLI`
    // (resolved pre-`build_runtime` above) sets `permission_notice` when the
    // bypass killswitch suppressed a requested bypass.
    if let Some(notice) = startup_deprecation_notice(&parsed) {
        eprintln!("{notice}");
    }

    // (T3) Terminal-compatibility notice: the fullscreen alt-screen UI ghosts on
    // Warp (non-standard alt-screen compositing — stacked frames / stray rows).
    // Print ONCE, before the alt-screen is entered, so it lands in Warp's
    // pre-alt-screen scrollback (Warp keeps it). Only for the interactive TUI
    // (print/REPL/stdio don't enter alt-screen, so they never ghost) and only to a
    // tty (no-op when stderr is piped). Standard terminals print nothing.
    if interactive_tui {
        use std::io::IsTerminal;
        if std::io::stderr().is_terminal() {
            if let Some(notice) =
                ghosting_terminal_notice(std::env::var("TERM_PROGRAM").ok().as_deref())
            {
                eprintln!("{notice}");
            }
        }
    }

    // (Item B) Permission-mode startup notice (TS `permissionModeNotification`,
    // `main.tsx:2882-2887`): set only when the bypass killswitch suppressed a
    // requested bypass (`initialPermissionModeFromCLI`). Emitted on the same
    // bounded stderr channel as the deprecation/migration notices (no UI
    // notification-queue substrate); `None` in the common case, so startup is
    // byte-identical when no bypass was disabled.
    if let Some(notice) = &permission_notice {
        eprintln!("{notice}");
    }

    let make_sink = || -> Arc<dyn output::OutputSink> {
        if parsed.is_json_output() {
            Arc::new(output::JsonSink::new(protocol::SessionId::new()))
        } else {
            Arc::new(output::PlainSink::new())
        }
    };

    // `-c/--continue` resumes the MOST-RECENT conversation in the current cwd's
    // project dir (claude-code `main.tsx`: `options.continue` →
    // `loadConversationForResume(undefined)`; errors `No conversation found to
    // continue` when none). Dispatched BEFORE `--resume` so the no-id continue
    // path is honored. `--continue --resume <id>` is rejected upstream
    // (lib.rs:315 cross-flag rule), so the two never collide here.
    if parsed.continue_session {
        let mut resumed_argv = parsed.clone();
        if let Some(effort) = run::inherited_resume_effort(&parsed).await {
            resumed_argv.effort = Some(effort);
        }
        let sink = make_sink();
        let adapter: Arc<dyn traits::OutputStream> =
            Arc::new(output_adapter::SinkAdapter::new(sink.clone()));
        let runtime = match init::build_runtime(&resumed_argv, adapter, permission_mode).await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("lingxi-cli: {e}");
                return exit_codes::RUNTIME_ERROR;
            }
        };
        return run::run_continue(&resumed_argv, &runtime, sink.as_ref()).await;
    }

    // --resume routes through run::run_resume, which itself splits (M7-12):
    //   <uuid>            → load by id
    //   (none) + TTY      → iocraft Resume screen
    //   (none) + --no-tui → M5-08 stdio picker (unchanged fallback)
    if parsed.resume.is_some() {
        let mut resumed_argv = parsed.clone();
        if let Some(effort) = run::inherited_resume_effort(&parsed).await {
            resumed_argv.effort = Some(effort);
        }
        let sink = make_sink();
        let adapter: Arc<dyn traits::OutputStream> =
            Arc::new(output_adapter::SinkAdapter::new(sink.clone()));
        let runtime = match init::build_runtime(&resumed_argv, adapter, permission_mode).await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("lingxi-cli: {e}");
                return exit_codes::RUNTIME_ERROR;
            }
        };
        return run::run_resume(&resumed_argv, &runtime, sink.as_ref()).await;
    }

    // (M4 cc2.1.198) `--from-pr [value]` — the binary opens the SAME resume
    // picker with `filterByPr: rt` (bare flag → only PR-linked sessions; a
    // parseable PR number/URL → `prNumber === n`; an unparseable value applies
    // no narrowing). `--resume` wins when both are given (its branch runs
    // first in the binary's session-source resolution too).
    if parsed.from_pr.is_some() {
        let sink = make_sink();
        return run::run_from_pr(&parsed, sink.as_ref()).await;
    }

    match chosen {
        mode::Mode::Tui | mode::Mode::StdioRepl => {
            mode::dispatch_interactive(chosen, &parsed).await
        }
        mode::Mode::Print(_) => {
            let sink = make_sink();
            let adapter: Arc<dyn traits::OutputStream> =
                Arc::new(output_adapter::SinkAdapter::new(sink.clone()));
            let runtime = match init::build_runtime(&parsed, adapter, permission_mode).await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("lingxi-cli: {e}");
                    return exit_codes::RUNTIME_ERROR;
                }
            };
            mode::dispatch(chosen, &parsed, &runtime, sink).await
        }
    }
}

/// Resolve the session permission mode (and any suppression notice) from CLI
/// flags + merged `settings.json` — the shared resolver every dispatch path
/// uses so the one-shot/print, interactive TUI, and stdio-REPL paths all see
/// the SAME `initialPermissionModeFromCLI` result.
///
/// This is `read_cli_mode_settings` + `permission::initial_permission_mode_from_cli`,
/// with NO bypass-safety guard: the guard runs exactly once in [`run_cli`]
/// (before mode dispatch, for all modes), so the interactive paths that call
/// this helper to re-derive the mode must NOT re-run it. Returns
/// `(mode, notice)` where `notice` is `Some` when the bypass killswitch
/// suppressed a requested bypass, OR when the auto-mode availability gate
/// downgraded a requested `auto` (`permissionModeNotification`).
///
/// The auto-mode gate (claude-code `xms` mode-load downgrade + the
/// `kickOutOfAutoIfNeeded` notification) runs AFTER the pure
/// `initialPermissionModeFromCLI` resolution: when the resolved mode is `Auto`
/// but auto mode is unavailable (`disableAutoMode` settings killswitch or the
/// active model does not support it), the mode is downgraded to `Default` and
/// the byte-exact `Jce()` reason (`"auto mode disabled by settings"` /
/// `"auto mode unavailable for this model"`) is surfaced as the startup notice.
/// The local denial circuit-breaker is fresh at boot (never tripped); Statsig
/// remote-disable is a documented omission. The provider is resolved as
/// `"firstParty"` at this CLI surface (multi-provider provider-mapping into the
/// gate is deferred — see [`permission::auto_gate`]).
pub(crate) fn resolve_permission_mode(argv: &Argv) -> (permission::PermissionMode, Option<String>) {
    let settings = read_cli_mode_settings(argv);
    // MODE-ENV-SCRUB-03: `LINGXI_SUBPROCESS_ENV_SCRUB` (the port's spelling of
    // `CLAUDE_CODE_SUBPROCESS_ENV_SCRUB`, `platforms/posix` runner) forces the
    // permission mode to `default` — a hardened / scrubbed subprocess must not
    // inherit a requested bypass/plan/etc.
    let env_scrub_active =
        traits::env::is_env_truthy(std::env::var("LINGXI_SUBPROCESS_ENV_SCRUB").ok().as_deref());
    // MODE-FRONTMATTER-04: the selected main-thread agent's frontmatter
    // `permissionMode` sits between the CLI override and the settings
    // `defaultMode`. The agent catalog is resolved later in
    // `engine_desktop::build()`, so this early CLI pass cannot see it yet; the
    // composition root re-applies the same precedence once it knows which
    // agent actually won.
    let agent_frontmatter_mode: Option<permission::PermissionMode> = None;
    let (mode, notice) = permission::initial_permission_mode_from_cli(
        argv.permission_mode.as_deref(),
        argv.dangerously_skip_permissions,
        agent_frontmatter_mode,
        env_scrub_active,
        &settings,
    );
    if mode != permission::PermissionMode::Auto {
        return (mode, notice);
    }
    // Auto was requested (CLI flag or settings `defaultMode: auto`). Apply the
    // availability gate; downgrade + notify when closed.
    let model = argv
        .model
        .clone()
        .unwrap_or_else(|| engine_desktop::DesktopConfig::default().default_model);
    let inputs = permission::AutoGateInputs {
        disabled_by_settings: settings.auto_mode_disabled,
        circuit_broken: false,
        model,
        provider: "firstParty".to_string(),
    };
    match permission::apply_auto_mode_gate(mode, &inputs) {
        (permission::PermissionMode::Auto, _) => (mode, notice),
        (downgraded, Some(reason)) => (downgraded, Some(reason.message().to_string())),
        (downgraded, None) => (downgraded, notice),
    }
}

/// Build [`permission::CliModeSettings`] from the merged user+project
/// `settings.json` files (the bypass-killswitch + settings `defaultMode`
/// inputs the mode resolver reads).
///
/// Reads the user `~/.lingxi/settings.json` then the project
/// `<cwd>/.lingxi/settings.json` raw, deriving the two fields via the existing
/// `permission` helpers: `defaultMode` takes project-wins precedence (project
/// read last), and the bypass-disable killswitch is sticky (set by any tier).
/// On any load failure (missing/unreadable/malformed file) it degrades to the
/// no-op default `{ default_mode: None, bypass_disabled: false }` — a faithful
/// port of TS `getSettings_DEPRECATED() || {}`.
///
/// Explicit `--settings` is read after ambient files so it remains effective
/// in restricted mode even while user/project/local files are suppressed.
pub(crate) fn read_cli_mode_settings(parsed: &Argv) -> permission::CliModeSettings {
    // `--setting-sources <user,project,local>` gates which settings files this
    // permission-mode reader consults too (claude scopes ALL settings loading,
    // not just providers/routing). `None` ⟶ both layers (default).
    let (incl_user, incl_project) = if parsed.restricted_enabled() {
        (false, false)
    } else {
        init::setting_source_flags(parsed.setting_sources.as_deref())
    };
    let project_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let mut default_mode = None;
    let mut bypass_disabled = false;
    let mut auto_mode_disabled = false;
    // MODE-SETTINGS-AUTO-TRUST-01: track whether a TRUSTED tier declared
    // `defaultMode: auto`. At this CLI surface only the user (`~/.lingxi`) tier
    // is trusted; the project (`.lingxi`) tier is repo-controllable. When the
    // merged `default_mode` ends up `auto` but no trusted tier granted it, the
    // resolver drops it (a committed project settings file cannot enable
    // classifier-driven auto-accept mode).
    let mut auto_default_from_trusted = false;
    // MODE-BG-DISCLAIMER-02: sticky across tiers (any tier accepting wins — `Pq()`).
    let mut skip_dangerous_mode_permission_prompt = false;
    let home = incl_user
        .then(|| crate::run::lingxi_home_dir().join("settings.json"))
        .map(|p| (p, permission::PermissionRuleSource::UserSettings));
    let proj = incl_project
        .then(|| project_dir.join(branding::DOT_DIR).join("settings.json"))
        .map(|p| (p, permission::PermissionRuleSource::ProjectSettings));
    // User first, then project (ascending priority): project read last wins on
    // `defaultMode`; `bypass_disabled` / `auto_mode_disabled` are sticky across
    // tiers (any tier disabling wins — `Bpa()`).
    for (path, source) in [home, proj].into_iter().flatten() {
        if let Ok(raw) = std::fs::read_to_string(&path) {
            if let Some(m) = permission::default_mode_from_settings_json(&raw) {
                default_mode = Some(m);
                if m == permission::PermissionMode::Auto
                    && permission::loader::auto_mode_grantable_by_source(source)
                {
                    auto_default_from_trusted = true;
                }
            }
            if permission::bypass_permissions_disabled_from_settings_json(&raw) {
                bypass_disabled = true;
            }
            if permission::auto_mode_disabled_from_settings_json(&raw) {
                auto_mode_disabled = true;
            }
            // `Pq()` reads `skipDangerousModePermissionPrompt` from
            // {userSettings, localSettings, flagSettings, policySettings} —
            // DELIBERATELY EXCLUDING projectSettings, so a repo-controllable
            // `.lingxi/settings.json` cannot suppress the bg-bypass disclaimer
            // downgrade (same repo-trust threat MODE-SETTINGS-AUTO-TRUST-01
            // guards). Only the user tier is loaded here; local/flag/policy are
            // not read at this surface (their omission is over-ask-safe).
            if source != permission::PermissionRuleSource::ProjectSettings
                && permission::loader::skip_dangerous_mode_permission_prompt_from_settings_json(
                    &raw,
                )
            {
                skip_dangerous_mode_permission_prompt = true;
            }
        }
    }
    if let Some(raw) = parsed
        .settings
        .as_deref()
        .and_then(|_| init::parse_flag_settings(parsed.settings.as_deref()))
        .and_then(|settings| serde_json::to_string(&settings).ok())
    {
        let source = permission::PermissionRuleSource::FlagSettings;
        if let Some(m) = permission::default_mode_from_settings_json(&raw) {
            default_mode = Some(m);
            if m == permission::PermissionMode::Auto
                && permission::loader::auto_mode_grantable_by_source(source)
            {
                auto_default_from_trusted = true;
            }
        }
        if permission::bypass_permissions_disabled_from_settings_json(&raw) {
            bypass_disabled = true;
        }
        if permission::auto_mode_disabled_from_settings_json(&raw) {
            auto_mode_disabled = true;
        }
        if permission::loader::skip_dangerous_mode_permission_prompt_from_settings_json(&raw) {
            skip_dangerous_mode_permission_prompt = true;
        }
    }
    // MODE-BG-DISCLAIMER-02: bg-session downgrade inputs. `is_bg_session` is
    // `LINGXI_SESSION_KIND == "bg"` (claude-code `CLAUDE_CODE_SESSION_KIND`);
    // `bypass_permissions_mode_accepted` is the persisted global-config flag
    // (`St().bypassPermissionsModeAccepted`), read best-effort (absent ⇒ false,
    // i.e. the gate may trip — over-ask safe).
    let is_bg_session = std::env::var("LINGXI_SESSION_KIND").ok().as_deref() == Some("bg");
    let bypass_permissions_mode_accepted = migrations::global_config::global_config_path()
        .and_then(|p| migrations::global_config::read_map(&p).ok())
        .and_then(|m| {
            m.get("bypassPermissionsModeAccepted")
                .and_then(|v| v.as_bool())
        })
        .unwrap_or(false);
    permission::CliModeSettings {
        default_mode,
        bypass_disabled,
        auto_mode_disabled,
        auto_default_from_trusted,
        is_bg_session,
        skip_dangerous_mode_permission_prompt,
        bypass_permissions_mode_accepted,
    }
}

/// Resolve the initial main-loop model the engine will use, then return the
/// model-deprecation startup notice for it (or `None` when current).
///
/// The model resolution is byte-identical to the one
/// `init::resolve_desktop_config` threads into `DesktopConfig.default_model`:
/// the `--model` override if present, else the desktop default
/// (`engine_desktop::DesktopConfig::default().default_model`). This mirrors
/// claude-code's `resolvedInitialModel = parseUserSpecifiedModel(
/// initialMainLoopModel ?? getDefaultMainLoopModel())` (`main.tsx:2116`), the
/// same value fed to `getModelDeprecationWarning` at `main.tsx:2873`.
///
/// The lookup itself is `engine_desktop::model_deprecation_warning`
/// (moved from the deleted `providers` crate in Plan 3b); it returns
/// `Some(warning)` only for a deprecated model under the active provider and
/// `None` otherwise — so a current default model yields `None` and the caller
/// prints nothing (byte-identical startup).
fn startup_deprecation_notice(argv: &Argv) -> Option<String> {
    let resolved_model = argv
        .model
        .clone()
        .unwrap_or_else(|| engine_desktop::DesktopConfig::default().default_model);
    engine_desktop::model_deprecation_warning(Some(&resolved_model))
}

/// (T3) One-line terminal-compatibility notice for terminals known to mis-render
/// the fullscreen alt-screen UI. Currently only Warp (`TERM_PROGRAM=WarpTerminal`)
/// — its non-standard alt-screen compositing ghosts (stacked frames). Standard
/// terminals (iTerm2, Terminal.app, Alacritty, Ghostty, kitty, …) render it
/// correctly, so they get `None`. Pure (env value injected) for unit-testing; the
/// REAL fix for Warp is an inline (non-alt-screen) render loop — a large change
/// the viewport math isn't built for, deferred.
fn ghosting_terminal_notice(term_program: Option<&str>) -> Option<&'static str> {
    match term_program {
        Some("WarpTerminal") => Some(
            "\u{26a0} Warp can ghost LingXi's full-screen UI (stacked frames / stray rows). \
             Use iTerm2, Terminal.app, Alacritty, Ghostty, or kitty \u{2014} or try the \
             experimental inline mode: LINGXI_TUI_INLINE=1.",
        ),
        _ => None,
    }
}

#[cfg(test)]
mod startup_notice_tests {
    use super::*;

    #[test]
    fn only_warp_gets_the_ghosting_notice() {
        assert!(ghosting_terminal_notice(Some("WarpTerminal")).is_some());
        assert!(ghosting_terminal_notice(Some("iTerm.app")).is_none());
        assert!(ghosting_terminal_notice(Some("Apple_Terminal")).is_none());
        assert!(ghosting_terminal_notice(Some("ghostty")).is_none());
        assert!(ghosting_terminal_notice(None).is_none());
    }

    fn argv_with_model(model: Option<&str>) -> Argv {
        Argv {
            model: model.map(String::from),
            ..Argv::default()
        }
    }

    /// SAFETY: the resolved DEFAULT model (no `--model`) is a current Claude 4
    /// id, so the deprecation lookup returns `None` and startup prints nothing —
    /// byte-identical to before this notice landed. This is the common-case
    /// invariant the brief pins.
    #[test]
    fn default_model_yields_no_notice() {
        // Belt-and-suspenders: assert against the actual desktop default rather
        // than a hardcoded literal so a future default bump can't silently start
        // emitting a notice at every startup.
        let default_model = engine_desktop::DesktopConfig::default().default_model;
        assert!(
            engine_desktop::model_deprecation_warning(Some(&default_model)).is_none(),
            "the shipped desktop default model ({default_model}) must not be deprecated, \
             else every startup would emit a notice"
        );
        assert_eq!(startup_deprecation_notice(&argv_with_model(None)), None);
    }

    /// A `--model` override naming a CURRENT model still yields no notice.
    #[test]
    fn current_override_model_yields_no_notice() {
        assert_eq!(
            startup_deprecation_notice(&argv_with_model(Some("claude-opus-4-7"))),
            None
        );
    }

    /// A `--model` override naming a DEPRECATED model surfaces the exact
    /// TS-faithful warning line (`deprecation.ts:100` text, byte-locked
    /// including the leading `⚠ ` glyph). This is the only condition under which
    /// startup emits anything. The provider env is cleared first so the
    /// first-party retirement date is the one asserted (the table carries a
    /// different date per provider; see `engine_desktop::model_deprecation_warning`).
    #[test]
    fn deprecated_override_model_yields_first_party_notice() {
        let prior = [
            (
                "CLAUDE_CODE_USE_BEDROCK",
                std::env::var_os("CLAUDE_CODE_USE_BEDROCK"),
            ),
            (
                "CLAUDE_CODE_USE_VERTEX",
                std::env::var_os("CLAUDE_CODE_USE_VERTEX"),
            ),
            (
                "CLAUDE_CODE_USE_FOUNDRY",
                std::env::var_os("CLAUDE_CODE_USE_FOUNDRY"),
            ),
        ];
        for (k, _) in &prior {
            std::env::remove_var(k);
        }

        let notice = startup_deprecation_notice(&argv_with_model(Some("claude-3-opus-20240229")))
            .expect("a deprecated --model must produce a startup notice");
        assert_eq!(
            notice,
            "⚠ Claude 3 Opus will be retired on January 5, 2026. Consider switching to a newer model."
        );

        // Restore any provider flags this test cleared.
        for (k, v) in prior {
            if let Some(v) = v {
                std::env::set_var(k, v);
            }
        }
    }
}

#[cfg(test)]
mod config_startup_tests {
    use super::{command_initializes_user_id, command_runs_config_startup};
    use crate::argv::Argv;

    fn parsed(args: &[&str]) -> Argv {
        Argv::from_iter(std::iter::once("lingxi-cli").chain(args.iter().copied())).unwrap()
    }

    #[test]
    fn standard_commands_run_startup_while_special_fast_paths_do_not() {
        for args in [
            &["auth", "status"][..],
            &["auto-mode", "defaults"][..],
            &["mcp", "list"][..],
            &["plugin", "list"][..],
            &["doctor"][..],
        ] {
            let argv = parsed(args);
            assert!(
                command_runs_config_startup(argv.command.as_ref()),
                "{args:?}"
            );
        }
        for args in [
            &["project"][..],
            &["attach", "missing"][..],
            &["remote-control"][..],
        ] {
            let argv = parsed(args);
            assert!(
                !command_runs_config_startup(argv.command.as_ref()),
                "{args:?}"
            );
        }
        let project_purge = parsed(&["project", "purge", "--dry-run", "--yes"]);
        assert!(command_runs_config_startup(project_purge.command.as_ref()));
        assert!(command_runs_config_startup(None));
    }

    #[test]
    fn only_device_identity_command_families_eagerly_create_user_id() {
        for args in [
            &["mcp", "list"][..],
            &["doctor"][..],
            &["setup-token"][..],
            &["install"][..],
            &["update"][..],
        ] {
            let argv = parsed(args);
            assert!(
                command_initializes_user_id(argv.command.as_ref()),
                "{args:?}"
            );
        }
        let argv = parsed(&["auth", "status"]);
        assert!(!command_initializes_user_id(argv.command.as_ref()));
    }
}

#[cfg(test)]
mod session_id_store_tests {
    use super::session_id_exists_in_store;
    use session::session_path;
    use uuid::Uuid;

    #[tokio::test]
    async fn detects_existing_session_id_in_another_project_dir() {
        let home = tempfile::tempdir().expect("config home");
        let project_a = tempfile::tempdir().expect("project a");
        let session_id = Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap();
        let transcript = session_path(
            home.path(),
            &project_a.path().display().to_string(),
            &session_id.to_string(),
        );
        std::fs::create_dir_all(transcript.parent().expect("project dir")).expect("mkdir");
        std::fs::write(&transcript, "").expect("write transcript");

        assert!(
            session_id_exists_in_store(home.path(), session_id)
                .await
                .expect("store scan"),
            "cross-project transcript lookup should find occupied ids anywhere under config_home/projects"
        );
    }

    #[tokio::test]
    async fn missing_projects_store_is_empty() {
        let home = tempfile::tempdir().expect("config home");
        assert!(!session_id_exists_in_store(home.path(), Uuid::new_v4())
            .await
            .expect("missing projects store"));
    }

    #[tokio::test]
    async fn unreadable_projects_store_fails_closed() {
        let home = tempfile::tempdir().expect("config home");
        std::fs::write(home.path().join("projects"), "not a directory").expect("sentinel file");
        let error = session_id_exists_in_store(home.path(), Uuid::new_v4())
            .await
            .expect_err("invalid projects store must not be treated as empty");
        assert_ne!(error.kind(), std::io::ErrorKind::NotFound);
    }
}

#[cfg(test)]
mod cli_mode_settings_tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    /// `read_cli_mode_settings` reads process-global state (`$HOME` via
    /// `dirs::home_dir` + the process cwd), so the two tests that mutate those
    /// must not run concurrently. A local mutex serializes them (the rest of the
    /// resolver is pure and tested env-free in `permission::cli_mode`).
    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn argv() -> Argv {
        Argv::default()
    }

    /// With no `~/.lingxi/settings.json` and no `<cwd>/.lingxi/settings.json`,
    /// the helper degrades to the no-op default (the faithful TS
    /// `getSettings_DEPRECATED() || {}` fallback).
    #[test]
    fn degrades_to_default_when_no_settings_files() {
        let _g = env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let prior_home = std::env::var_os("HOME");
        let prior_cwd = std::env::current_dir().ok();

        // Point HOME + cwd at fresh empty dirs (no `.lingxi/settings.json`).
        let home = tempfile::tempdir().expect("home tempdir");
        let proj = tempfile::tempdir().expect("proj tempdir");
        std::env::set_var("HOME", home.path());
        std::env::set_current_dir(proj.path()).expect("chdir proj");

        let s = read_cli_mode_settings(&argv());
        assert!(s.default_mode.is_none());
        assert!(!s.bypass_disabled);

        // Restore process-global state.
        if let Some(cwd) = prior_cwd {
            let _ = std::env::set_current_dir(cwd);
        }
        match prior_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }

    /// The project `<cwd>/.lingxi/settings.json` `defaultMode` is read and wins
    /// over the user tier, and `disableBypassPermissionsMode: "disable"` sets the
    /// killswitch — exercising the parse path, not just the empty degrade.
    #[test]
    fn reads_project_settings_default_mode_and_killswitch() {
        let _g = env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let prior_home = std::env::var_os("HOME");
        let prior_cwd = std::env::current_dir().ok();

        let home = tempfile::tempdir().expect("home tempdir");
        let proj = tempfile::tempdir().expect("proj tempdir");
        let proj_lingxi = proj.path().join(".lingxi");
        std::fs::create_dir_all(&proj_lingxi).expect("mkdir .lingxi");
        std::fs::write(
            proj_lingxi.join("settings.json"),
            r#"{"permissions":{"defaultMode":"acceptEdits","disableBypassPermissionsMode":"disable"}}"#,
        )
        .expect("write settings");
        std::env::set_var("HOME", home.path());
        std::env::set_current_dir(proj.path()).expect("chdir proj");

        let s = read_cli_mode_settings(&argv());
        assert_eq!(
            s.default_mode,
            Some(permission::PermissionMode::AcceptEdits)
        );
        assert!(s.bypass_disabled);

        if let Some(cwd) = prior_cwd {
            let _ = std::env::set_current_dir(cwd);
        }
        match prior_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }

    /// A settings `disableAutoMode: "disable"` (either position) sets the
    /// auto-mode killswitch in the resolved [`permission::CliModeSettings`].
    #[test]
    fn reads_auto_mode_killswitch() {
        let _g = env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let prior_home = std::env::var_os("HOME");
        let prior_cwd = std::env::current_dir().ok();

        let home = tempfile::tempdir().expect("home tempdir");
        let proj = tempfile::tempdir().expect("proj tempdir");
        let proj_lingxi = proj.path().join(".lingxi");
        std::fs::create_dir_all(&proj_lingxi).expect("mkdir .lingxi");
        std::fs::write(
            proj_lingxi.join("settings.json"),
            r#"{"permissions":{"disableAutoMode":"disable"}}"#,
        )
        .expect("write settings");
        std::env::set_var("HOME", home.path());
        std::env::set_current_dir(proj.path()).expect("chdir proj");

        let s = read_cli_mode_settings(&argv());
        assert!(s.auto_mode_disabled);

        if let Some(cwd) = prior_cwd {
            let _ = std::env::set_current_dir(cwd);
        }
        match prior_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }

    /// `--permission-mode auto` + `disableAutoMode: "disable"` → the session
    /// boots `Default` with the byte-exact `auto mode disabled by settings`
    /// notice (claude-code `xms` downgrade + `Jce("settings")`).
    #[test]
    fn resolve_downgrades_auto_when_disabled_by_settings() {
        let _g = env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let prior_home = std::env::var_os("HOME");
        let prior_cwd = std::env::current_dir().ok();

        let home = tempfile::tempdir().expect("home tempdir");
        let proj = tempfile::tempdir().expect("proj tempdir");
        let proj_lingxi = proj.path().join(".lingxi");
        std::fs::create_dir_all(&proj_lingxi).expect("mkdir .lingxi");
        std::fs::write(
            proj_lingxi.join("settings.json"),
            r#"{"disableAutoMode":"disable"}"#,
        )
        .expect("write settings");
        std::env::set_var("HOME", home.path());
        std::env::set_current_dir(proj.path()).expect("chdir proj");

        let mut a = argv();
        a.permission_mode = Some("auto".to_string());
        let (mode, notice) = resolve_permission_mode(&a);
        assert_eq!(mode, permission::PermissionMode::Default);
        assert_eq!(notice.as_deref(), Some("auto mode disabled by settings"));

        if let Some(cwd) = prior_cwd {
            let _ = std::env::set_current_dir(cwd);
        }
        match prior_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }

    /// `--permission-mode auto` on an auto-UNSUPPORTED model (no settings) →
    /// boots `Default` with `auto mode unavailable for this model`
    /// (`Jce("model")`, the `dUe` deny-list).
    #[test]
    fn resolve_downgrades_auto_on_unsupported_model() {
        let _g = env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let prior_home = std::env::var_os("HOME");
        let prior_cwd = std::env::current_dir().ok();

        // Empty settings dirs (no killswitch) — the ONLY closed gate is the model.
        let home = tempfile::tempdir().expect("home tempdir");
        let proj = tempfile::tempdir().expect("proj tempdir");
        std::env::set_var("HOME", home.path());
        std::env::set_current_dir(proj.path()).expect("chdir proj");

        let mut a = argv();
        a.permission_mode = Some("auto".to_string());
        a.model = Some("claude-sonnet-4-5".to_string()); // legacy → auto-unsupported
        let (mode, notice) = resolve_permission_mode(&a);
        assert_eq!(mode, permission::PermissionMode::Default);
        assert_eq!(
            notice.as_deref(),
            Some("auto mode unavailable for this model")
        );

        if let Some(cwd) = prior_cwd {
            let _ = std::env::set_current_dir(cwd);
        }
        match prior_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }
}

#[cfg(test)]
mod help_layout_tests {
    use super::{normalise_preamble, reorder_help_sections};

    /// clap renders subcommand help description-first with Commands before
    /// Options; commander renders Usage-first with
    /// Arguments -> Options -> Commands.
    const CLAP_STYLE: &str = "\
Configure and manage MCP servers

Usage: lingxi-cli mcp [COMMAND]

Commands:
  add   Add a server
  list  List servers

Options:
  -h, --help  Print help
";

    #[test]
    fn usage_moves_ahead_of_the_description() {
        let out = normalise_preamble(CLAP_STYLE);
        let first = out.lines().next().unwrap();
        assert!(first.starts_with("Usage:"), "{out}");
        assert!(out.contains("Configure and manage MCP servers"), "{out}");
    }

    #[test]
    fn a_wrapped_usage_block_moves_whole() {
        // Taking only the first line would strip the wrapped tail onto the
        // wrong side of the description.
        let input =
            "Some description\n\nUsage: cli foo [OPTIONS]\n           [EXTRA]\n\nOptions:\n  -h\n";
        let out = normalise_preamble(input);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "Usage: cli foo [OPTIONS]");
        assert_eq!(lines[1], "           [EXTRA]");
    }

    #[test]
    fn sections_are_reordered_to_commander_order() {
        let out = reorder_help_sections(CLAP_STYLE);
        let opts = out.find("Options:").expect("options");
        let cmds = out.find("Commands:").expect("commands");
        assert!(opts < cmds, "Options must precede Commands:\n{out}");
    }

    #[test]
    fn arguments_precede_options() {
        let input = "Usage: cli x\n\nOptions:\n  -h\n\nArguments:\n  [target]  A target\n";
        let out = reorder_help_sections(input);
        let args = out.find("Arguments:").expect("arguments");
        let opts = out.find("Options:").expect("options");
        assert!(args < opts, "Arguments must precede Options:\n{out}");
    }

    #[test]
    fn a_command_without_positionals_gets_no_arguments_header() {
        // The reason this is a text transform and not a help_template: a
        // template spelling the sections out would print a bare `Arguments:`
        // for the ~40 subcommands that have none.
        let out = reorder_help_sections(CLAP_STYLE);
        assert!(!out.contains("Arguments:"), "{out}");
    }

    #[test]
    fn exactly_one_blank_line_precedes_each_header() {
        let out = reorder_help_sections(CLAP_STYLE);
        for header in ["Options:", "Commands:"] {
            let at = out.find(header).expect(header);
            assert!(out[..at].ends_with("\n\n"), "{header} spacing:\n{out:?}");
            assert!(!out[..at].ends_with("\n\n\n"), "{header} doubled:\n{out:?}");
        }
    }

    #[test]
    fn an_unrecognised_section_is_not_dropped() {
        let input = "Usage: cli x\n\nOptions:\n  -h\n\nExamples:\n  cli x --yes\n";
        let out = reorder_help_sections(input);
        assert!(out.contains("Examples:"), "{out}");
        assert!(out.contains("cli x --yes"), "{out}");
    }

    #[test]
    fn help_without_any_section_is_returned_unchanged() {
        let input = "Usage: cli x\n\njust a description\n";
        assert_eq!(reorder_help_sections(input), input);
    }
}
