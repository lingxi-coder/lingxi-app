use crate::argv::Argv;
use crate::exit_codes;
use harness_runtime::headless::output::OutputSink;
use lingxi_core::host::{FileSystem, OrchestratorHandle};
use lingxi_core::types::utf16_json::Utf16JsonProjection;
use permission;
#[cfg(unix)]
use platform_posix::PosixFileSystem as HostFileSystem;
#[cfg(windows)]
use platform_windows::WindowsFileSystem as HostFileSystem;
use session::jsonl::loader::{LoaderError, SessionMetadata};
use session::jsonl::JsonlMessage;
use std::path::PathBuf;
use std::sync::Arc;

/// Create a branch without a previously captured source.
#[cfg(test)]
pub(crate) async fn create_independent_cli_branch(
    home: &std::path::Path,
    target_cwd: &str,
    source_id: uuid::Uuid,
    target_id: uuid::Uuid,
    selected_source_path: Option<&std::path::Path>,
    custom_title: Option<&str>,
) -> Result<session::branch::PublishedBranch, String> {
    create_independent_cli_branch_from_source(
        home,
        target_cwd,
        source_id,
        target_id,
        selected_source_path,
        custom_title,
        None,
    )
    .await
}

/// Capture a source under its writer claim, validate that it has no unfinished
/// recovery, then atomically publish a separate profile and transcript.
pub(crate) async fn create_independent_cli_branch_from_source(
    home: &std::path::Path,
    target_cwd: &str,
    source_id: uuid::Uuid,
    target_id: uuid::Uuid,
    selected_source_path: Option<&std::path::Path>,
    custom_title: Option<&str>,
    captured: Option<&crate::init::PublishedBranchSource>,
) -> Result<session::branch::PublishedBranch, String> {
    let path = match selected_source_path {
        Some(path) => path.to_path_buf(),
        None => crate::init::resolve_unique_cli_resume_path(home, source_id)
            .map_err(|error| error.to_string())?,
    };
    let path =
        crate::init::absolute_resolved_tui_resume_path(path).map_err(|error| error.to_string())?;
    let path = crate::init::validate_exact_tui_resume_path(home, source_id, &path)
        .map_err(|error| error.to_string())?;
    let fs: Arc<dyn FileSystem> = Arc::new(HostFileSystem::new(PathBuf::from(target_cwd)));
    let (preflight, source_identity) = crate::init::read_pinned_cli_resume_source(home, &path)
        .map_err(|error| error.to_string())?;
    if let Some(captured) = captured {
        if captured.authority.path() != path
            || captured
                .authority
                .read_checked()
                .map_err(|error| error.to_string())?
                != preflight.as_bytes()
            || captured.content != preflight
        {
            return Err("selected source changed before branch capture".into());
        }
    }
    let authority = match captured {
        Some(source) => source.authority.clone(),
        None => {
            let source = crate::init::capture_selected_cli_source(home, source_id, &path, None)
                .map_err(|error| error.to_string())?;
            let (_, identity) = crate::init::read_pinned_cli_resume_source(home, &path)
                .map_err(|error| error.to_string())?;
            if identity != source_identity || source.content != preflight {
                return Err("selected source changed before branch capture".into());
            }
            source.authority
        }
    };
    session::jsonl::loader::route_captured_session_source(&path, source_id, &preflight)
        .map_err(|error| error.to_string())?;
    crate::init::validate_resume_mode(
        &preflight,
        source_id,
        Some(lingxi_core::host::session_context::ConversationMode::Code),
    )
    .map_err(|error| error.to_string())?;
    let source_profile = session::JsonlWriter::fixed_session_profile_from_source(
        &preflight,
        &source_id.to_string(),
    )?;
    let live = lingxi_core::host::live_sessions::LiveSessionDir::at_live(home.join("sessions"));
    let source_lease = live
        .claim_session_id(&source_id.to_string(), std::process::id())
        .map_err(|error| format!("claim source session: {error}"))?
        .into_shared();
    let _target_lease = live
        .claim_session_id(&target_id.to_string(), std::process::id())
        .map_err(|error| format!("claim branch session: {error}"))?
        .into_shared();
    let (content, claimed_identity) = crate::init::read_pinned_cli_resume_source(home, &path)
        .map_err(|error| error.to_string())?;
    if claimed_identity != source_identity
        || authority
            .read_checked()
            .map_err(|error| error.to_string())?
            != content.as_bytes()
    {
        return Err("resume transcript changed during writer claim".into());
    }
    if captured.is_some_and(|source| source.content != content) {
        return Err("selected source changed during writer claim; select it again".into());
    }
    authority
        .verify_current()
        .map_err(|error| format!("resume transcript changed during writer claim: {error}"))?;
    let loaded = session::jsonl::loader::route_captured_session_source(&path, source_id, &content)
        .map_err(|error| error.to_string())?;
    crate::init::validate_resume_mode(
        &content,
        source_id,
        Some(lingxi_core::host::session_context::ConversationMode::Code),
    )
    .map_err(|error| error.to_string())?;
    if session::JsonlWriter::fixed_session_profile_from_source(&content, &source_id.to_string())?
        != source_profile
    {
        return Err("session profile changed during branch capture".into());
    }
    let (messages, _) = session::jsonl::build_conversation_chain(&loaded, &source_id.to_string());
    let recovery = harness_runtime::desktop::session_state::capture_branch_recovery(
        home,
        source_id,
        source_lease.clone(),
    )
    .await
    .map_err(|error| format!("resume the original conversation first: {error}"))?;
    orchestrator::native_computer::validate_independent_branch(
        source_id,
        &content,
        &messages,
        recovery.as_ref(),
    )
    .map_err(|error| format!("resume the original conversation first: {error}"))?;
    let source_cwd = session::jsonl::loader::transcript_workspace_path(&path, &loaded)
        .unwrap_or_else(|| target_cwd.to_owned());
    let published = session::branch::create_branch_from_captured(
        session::branch::CapturedBranch {
            lingxi_home: home,
            source_cwd: &source_cwd,
            target_cwd,
            source_session_id: source_id,
            target_session_id: target_id,
            source_content: &content,
            messages: &messages,
            custom_title,
            mode: Some(lingxi_core::host::session_context::ConversationMode::Code),
        },
        fs,
    )
    .await
    .map_err(|error| error.to_string())?;
    crate::init::validate_exact_tui_resume_path(home, target_id, &published.authority.path())
        .map_err(|error| error.to_string())?;
    Ok(published)
}

async fn initial_resume_target(
    argv: &Argv,
    source_id: uuid::Uuid,
    source_path: PathBuf,
    picked_source: Option<crate::init::PublishedBranchSource>,
) -> Result<
    (
        uuid::Uuid,
        PathBuf,
        Option<crate::init::PublishedBranchSource>,
    ),
    String,
> {
    let captured = match picked_source {
        Some(captured) => {
            let selected_path = crate::init::validate_exact_tui_resume_path(
                &lingxi_home_dir(),
                source_id,
                &source_path,
            )
            .map_err(|error| error.to_string())?;
            if captured.authority.path() != selected_path
                || captured
                    .authority
                    .read_checked()
                    .map_err(|error| error.to_string())?
                    != captured.content.as_bytes()
            {
                return Err("The selected session changed; select it again".into());
            }
            captured
        }
        None => crate::init::capture_selected_cli_source(
            &lingxi_home_dir(),
            source_id,
            &source_path,
            Some(lingxi_core::host::session_context::ConversationMode::Code),
        )
        .map_err(|error| error.to_string())?,
    };
    let source_path = captured.authority.path();
    if !argv.fork_session {
        return Ok((source_id, source_path, Some(captured)));
    }
    let target_id = match argv.session_id.as_deref() {
        Some(id) => lingxi_core::types::SessionId::parse_prefixed(id)
            .map(|id| id.as_uuid())
            .ok_or_else(|| "Invalid branch session ID".to_owned())?,
        None => uuid::Uuid::new_v4(),
    };
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    let published = create_independent_cli_branch_from_source(
        &lingxi_home_dir(),
        &cwd.to_string_lossy(),
        source_id,
        target_id,
        Some(&source_path),
        None,
        Some(&captured),
    )
    .await?;
    Ok((
        target_id,
        published.authority.path(),
        Some(published.into()),
    ))
}

/// Where a `--resume` invocation should be handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeRoute {
    /// `--resume <uuid>` — load that concrete session.
    LoadById,
    /// `--resume` (no id), TTY, no `--no-tui` → iocraft Resume screen (M7-12).
    IocraftScreen,
    /// `--resume` (no id), `--no-tui` or non-TTY → M5-08 stdio picker.
    StdioPicker,
}

/// Decide how to handle a `--resume` invocation. Pure (TTY passed in).
#[must_use]
pub fn resume_route(argv: &Argv, is_tty: bool) -> ResumeRoute {
    let arg = argv.resume.as_deref().unwrap_or("");
    if !arg.is_empty() {
        return ResumeRoute::LoadById;
    }
    if argv.no_tui || !is_tty {
        ResumeRoute::StdioPicker
    } else {
        ResumeRoute::IocraftScreen
    }
}

/// Resume an existing session.
///
/// (M7-12) Splits three ways on [`resume_route`]: a concrete id loads by id,
/// an empty arg under a full TTY opens the iocraft Resume screen, and an empty
/// arg under `--no-tui` / a non-TTY falls back to the unchanged M5-08 stdio
/// picker.
pub async fn run_resume(argv: &Argv, sink: &dyn OutputSink) -> i32 {
    match resume_route(argv, crate::mode::is_full_tty()) {
        ResumeRoute::LoadById => run_resume_by_id(argv, sink).await,
        ResumeRoute::IocraftScreen => run_resume_iocraft(argv, sink).await,
        ResumeRoute::StdioPicker => run_resume_stdio_picker(argv, sink).await,
    }
}

/// (M4 cc2.1.198) `--from-pr [value]` — resume a session linked to a PR.
///
/// The binary routes this through the SAME interactive resume picker as a
/// bare `--resume`, passing `filterByPr: rt` (main action: `if(a.fromPr){
/// if(a.fromPr===!0)rt=!0;else if(typeof a.fromPr==="string")rt=a.fromPr}` →
/// picker props `{…, initialSearchQuery: gc, forkSession: a.forkSession,
/// filterByPr: rt}`). There is NO by-id fast path: even `--from-pr 123` opens
/// the picker filtered to `prNumber === 123`. So the route here is the picker
/// split only (TTY → iocraft screen, `--no-tui`/non-TTY → stdio picker), with
/// [`filter_rows_by_pr`] applied to the loaded rows by both pickers.
pub async fn run_from_pr(argv: &Argv, sink: &dyn OutputSink) -> i32 {
    if argv.no_tui || !crate::mode::is_full_tty() {
        run_resume_stdio_picker(argv, sink).await
    } else {
        run_resume_iocraft(argv, sink).await
    }
}

/// (M4 cc2.1.198) Port of the picker's `filterByPr` row filter (`ne = k.filter
/// ((be)=>!be.isSidechain)` then `if(f===!0)…prNumber!==void 0; else if(typeof
/// f==="number")…prNumber===f; else if(typeof f==="string"){let be=wqc(f);
/// if(be!==null)…prNumber===be}`):
///
/// * `None` (no `--from-pr`) → rows unchanged;
/// * bare flag (`""`) → only PR-linked sessions;
/// * a value parsing to a PR number ([`parse_pr_value`]) → `prNumber === n`;
/// * an unparseable value → NO narrowing (the binary applies no filter).
///
pub(super) fn filter_rows_by_pr(
    rows: Vec<SessionMetadata>,
    from_pr: Option<&str>,
) -> Vec<SessionMetadata> {
    let Some(raw) = from_pr else { return rows };
    if raw.is_empty() {
        return rows
            .into_iter()
            .filter(|row| row.pr_number.is_some())
            .collect();
    }
    match parse_pr_value(raw) {
        Some(number) => rows
            .into_iter()
            .filter(|row| row.pr_number == Some(number))
            .collect(),
        // Unparseable value: the binary applies NO narrowing.
        None => rows,
    }
}

/// (M4 cc2.1.198) Port of `wqc` (2.1.198): `parseInt(e,10)` when `> 0`, else
/// the PR-URL match `/(?:https?:\/\/)?[^/\s]+\/[^\s]+?\/(?:pull|
/// pull-requests|-\/merge_requests)\/(\d+)/` (GitHub / Bitbucket / GitLab
/// forms), else `None`.
#[must_use]
pub(super) fn parse_pr_value(raw: &str) -> Option<u64> {
    session::jsonl::parse_pr_number(raw)
}

/// `--resume <uuid>` — the concrete-id path.
///
/// Parses the arg as a UUID, then (SESSION.4) verifies the session actually
/// exists on disk via the cross-worktree session loader BEFORE reporting success: a valid-but-
/// unknown id errors with the TS "No conversation found with session ID: {id}"
/// line and a non-zero exit instead of a false "Resumed session {id}".
///
/// Once confirmed present the dispatch mirrors the FRESH launch's
/// [`crate::mode::decide_mode`]:
///   - else under a full TTY (no `--no-tui`) → mount the live TUI with the
///     prior conversation replayed (M5-13 — [`mount_resumed_tui`]);
///   - else (`--no-tui` / non-TTY, no prompt) → keep the stdio fallback:
///     surface "Resumed session {id}" + the not-yet-wired stdio REPL notice.
pub(super) async fn run_resume_by_id(argv: &Argv, sink: &dyn OutputSink) -> i32 {
    // `t.resume.trim()` — the oracle trims before deciding id-vs-title.
    let arg = argv.resume.as_deref().unwrap_or("").trim();
    let (session_id, selected_path) = match resolve_session_id(arg) {
        Ok(id) => (id, None),
        // Not a UUID: fall back to a session-TITLE lookup before failing.
        Err(uuid_error) => match resolve_resume_title_source(arg).await {
            Ok(Some(row)) => (row.uuid, Some(row.path)),
            Ok(None) => {
                // Empty arg keeps the original uuid-parse error — there is no
                // title to look up and the oracle's title copy would misdescribe
                // it.
                sink.error("runtime", &uuid_error.to_string()).await;
                return exit_codes::RUNTIME_ERROR;
            }
            Err(message) => {
                sink.error("runtime", &message).await;
                return exit_codes::RUNTIME_ERROR;
            }
        },
    };
    resume_resolved_session(argv, sink, session_id, selected_path).await
}

/// Resolve `--resume <title>` to a session id — the `!s && i` arm of the
/// oracle's print entrypoint (2.1.220 @246508120):
///
/// ```js
/// let u = await OEe(i, {exact:!0});
/// if (u.length === 1) { let d = zS(u[0]); if (d) s = Cbi(d) }
/// else if (u.length > 1) { …"matches N sessions. Pass one of these session IDs to disambiguate:" }
/// ```
///
/// EXACT matching — `--resume` passes `{exact:!0}`, unlike the `/resume`
/// argument-completer which substring-matches with a limit of 10. Searching a
/// substring here would resume an arbitrary session on a partial title.
///
/// Returns `Ok(None)` for an empty argument (nothing to search), `Ok(Some(id))`
/// on a unique hit, and `Err(message)` for a no-match, an ambiguous match, or a
/// real catalog I/O failure. A genuinely empty catalog still follows the
/// no-match copy; unreadable/corrupt storage must not masquerade as one.
#[cfg(test)]
pub(crate) async fn resolve_resume_title(arg: &str) -> Result<Option<uuid::Uuid>, String> {
    Ok(resolve_resume_title_source(arg).await?.map(|row| row.uuid))
}

pub(crate) async fn resolve_resume_title_source(
    arg: &str,
) -> Result<Option<SessionMetadata>, String> {
    if arg.is_empty() {
        return Ok(None);
    }
    // The oracle loads EVERY log for the project before filtering; the picker's
    // 5-row cap is a display limit and must not silently bound the search.
    let rows = match load_resume_rows_all().await {
        Ok(rows) => rows,
        Err(LoaderError::EmptyDirectory) => Vec::new(),
        Err(error) => return Err(error.to_string()),
    };
    resolve_resume_title_source_from(rows, arg)
}

/// The pure resolution half of [`resolve_resume_title_source`], split out so the
/// one-match / no-match / ambiguous arms are unit-testable without touching the
/// environment or the process cwd — the same split [`load_resume_rows_from`]
/// exists for.
#[cfg(test)]
pub(super) fn resolve_resume_title_from(
    rows: Vec<SessionMetadata>,
    arg: &str,
) -> Result<Option<uuid::Uuid>, String> {
    Ok(resolve_resume_title_source_from(rows, arg)?.map(|row| row.uuid))
}

pub(crate) fn resolve_resume_title_source_from(
    rows: Vec<SessionMetadata>,
    arg: &str,
) -> Result<Option<SessionMetadata>, String> {
    if arg.is_empty() {
        return Ok(None);
    }
    let matches = session::jsonl::search_sessions_by_custom_title(rows, arg, true, None);
    match matches.len() {
        0 => Err(resume_title_not_found(arg)),
        1 => Ok(matches.into_iter().next()),
        _ => Err(resume_title_ambiguous(arg, &matches)),
    }
}

/// `--resume <title>` matched nothing. Oracle (@246508120):
///
/// ```js
/// let u = "Error: --resume requires a valid session ID or session title when used with
///          --print. Usage: claude -p --resume <session-id|title>";
/// if (i) u += `. Provided value "${i}" is not a UUID and does not match any session title.`;
/// ```
///
/// The base sentence carries no trailing period — the appended clause supplies
/// it. Only the binary name is adapted (`lingxi`, matching the "Resume with:"
/// hint); the product this port ships is not `claude`.
pub(super) fn resume_title_not_found(arg: &str) -> String {
    format!(
        "Error: --resume requires a valid session ID or session title when used with --print. \
         Usage: lingxi -p --resume <session-id|title>. Provided value \"{arg}\" is not a UUID \
         and does not match any session title."
    )
}

/// `--resume <title>` matched more than one session. Oracle (@246508120):
///
/// ```js
/// let d = u.map((p) => `  ${zS(p) ?? "(unknown)"}  (modified ${p.modified.toISOString()})`)
///          .join(`\n`);
/// `Error: --resume "${i}" matches ${u.length} sessions. Pass one of these session IDs to disambiguate:\n${d}`
/// ```
///
/// TWO spaces lead each row and TWO separate id from `(modified …)`. Rows arrive
/// newest-first from [`session::jsonl::search_sessions_by_custom_title`].
pub(super) fn resume_title_ambiguous(arg: &str, matches: &[SessionMetadata]) -> String {
    let listed = matches
        .iter()
        .map(|row| {
            let millis = row
                .modified
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_millis());
            format!(
                "  {}  (modified {})",
                row.uuid,
                session::jsonl::format_iso_millis(millis)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Error: --resume \"{arg}\" matches {} sessions. Pass one of these session IDs to \
         disambiguate:\n{listed}",
        matches.len()
    )
}

/// Every resumable row for the cwd's project, unbounded — the search corpus for
/// [`resolve_resume_title_source`] and the backing catalog for the paged picker.
pub(super) async fn load_resume_rows_all() -> Result<Vec<SessionMetadata>, LoaderError> {
    load_resume_rows().await
}

/// `-c/--continue` — resume the MOST-RECENT conversation in the current cwd's
/// project dir (claude-code `main.tsx`: `options.continue` →
/// `loadConversationForResume(undefined)` → newest log). When the project has no
/// resumable conversation, error with the byte-exact `No conversation found to
/// continue` and exit non-zero (TS `exitWithError`). Once a session is picked the
/// dispatch is identical to `--resume <uuid>` (prompt one-shot / TUI / stdio).
pub async fn run_continue(argv: &Argv, sink: &dyn OutputSink) -> i32 {
    // Newest-first rows over the cwd's project dir (same loader the picker uses);
    // `EmptyDirectory` (or an empty list) ⇒ nothing to continue.
    let rows = match load_resume_rows().await {
        Ok(rows) => rows,
        Err(LoaderError::EmptyDirectory) => {
            report_empty_continue(argv, sink).await;
            return exit_codes::RUNTIME_ERROR;
        }
        Err(e) => {
            sink.error("runtime", &e.to_string()).await;
            return exit_codes::RUNTIME_ERROR;
        }
    };
    let Some(first) = rows.first() else {
        report_empty_continue(argv, sink).await;
        return exit_codes::RUNTIME_ERROR;
    };
    resume_resolved_session(argv, sink, first.uuid, Some(first.path.clone())).await
}

async fn report_empty_continue(argv: &Argv, sink: &dyn OutputSink) {
    if !argv.no_tui && crate::mode::is_full_tty() {
        eprintln!("No conversation found to continue");
    } else {
        sink.error("runtime", "No conversation found to continue")
            .await;
    }
}

/// Shared post-resolution resume dispatch for both `--resume <uuid>` and
/// `--continue`: confirm the session exists on disk, then mirror the fresh launch
/// (prompt one-shot → TUI mount → stdio fallback).
pub(super) async fn resume_resolved_session(
    argv: &Argv,
    sink: &dyn OutputSink,
    session_id: uuid::Uuid,
    selected_path: Option<PathBuf>,
) -> i32 {
    let source_path = match selected_path {
        Some(path) => path,
        None => match crate::init::resolve_unique_cli_resume_path(&lingxi_home_dir(), session_id) {
            Ok(path) => path,
            Err(error) => {
                let message = if error.kind() == std::io::ErrorKind::NotFound {
                    format!("No conversation found with session ID: {session_id}")
                } else {
                    error.to_string()
                };
                sink.error("runtime", &message).await;
                return exit_codes::RUNTIME_ERROR;
            }
        },
    };
    let (session_id, source_path, published) =
        match initial_resume_target(argv, session_id, source_path, None).await {
            Ok(target) => target,
            Err(error) => {
                sink.error("runtime", &error).await;
                return exit_codes::RUNTIME_ERROR;
            }
        };
    // SESSION.4 parity: a resume for a session that does NOT exist on disk must
    // NOT report success. TS (claude-code/src/main.tsx:3675-3681) calls
    // `loadConversationForResume(sessionId)` and, when it yields nothing, exits
    // via `exitWithError(root, "No conversation found with session ID:
    // {sessionId}")` (exit code 1). We mirror that by loading the session up
    // front and only proceeding once it is confirmed to exist and parse.
    let loaded = load_resume_snapshot(
        session_id,
        published.as_ref().expect("resume source was captured"),
    );
    if let Some((message, code)) = resume_by_id_error(session_id, loaded.as_ref()) {
        sink.error("runtime", &message).await;
        return code;
    }
    // Session exists and parsed — these are the compacted resumable transcript
    // lines used to seed the orchestrator. The TUI render path reloads the full
    // routed entry stream below so pre-compaction rows remain visible.
    let messages = loaded.unwrap_or_default();

    // No prompt: mirror the fresh interactive dispatch. Under a full TTY (and no
    // `--no-tui`) run the same trust + dangerous-bypass acknowledgement gates
    // as a fresh TUI before mounting the resumed conversation.
    if crate::mode::is_full_tty() && !argv.no_tui {
        // Drive the mount, following any in-session `/resume` switch by
        // re-mounting the chosen session in-process (writer retargeted) until
        // the user quits — never an in-place `resume_session` swap. Cold
        // `--resume` carries no in-session model (`None`) — it opens on the
        // config/CLI model, matching claude-code's `--resume`.
        let first =
            mount_resumed_tui(argv, session_id, messages, source_path, None, published).await;
        return drive_tui_switch_loop(argv, first, Some(session_id), None).await;
    }

    sink.text(&format!("Resumed session {session_id}\n")).await;
    crate::repl::run_repl_resumed(argv, session_id, source_path, published).await
}

/// (M5-13) Mount the live TUI for a resumed `--resume <uuid>` session, seeded
/// with the prior conversation.
///
/// Reuses the FRESH TUI mount end-to-end ([`crate::init::build_runtime_for_tui`]
/// → [`crate::mode::run_ratatui`]), adding exactly the two resume seeds the W38
/// seam + the engine resume path expose:
///   1. ENGINE side — overwrite the freshly-built orchestrator's in-memory
///      `SessionState` (`history` + `session_id`) with the replayed transcript
///      via [`seed_orchestrator_session`], so a follow-up turn continues the
///      prior conversation rather than starting empty.
///   2. RENDER side — seed the TUI scrollback via
///      `tui::replay::rebuild_from_jsonl(&messages)`, so the existing history is
///      painted on the very first frame (the claude-code REPL `initialMessages`
///      analog).
///
/// A FRESH launch never reaches here; the fresh `Mode::Tui` arm calls
/// `build_tui_runtime` with an empty replay vec, so this change leaves the fresh
/// path byte-identical.
///
pub(super) async fn mount_resumed_tui(
    argv: &Argv,
    session_id: uuid::Uuid,
    messages: Vec<JsonlMessage>,
    transcript_path: PathBuf,
    carried_state: Option<crate::mode::RemountState>,
    published_branch: Option<crate::init::PublishedBranchSource>,
) -> crate::mode::RunOutcome {
    mount_resumed_tui_inner(
        argv,
        session_id,
        messages,
        carried_state,
        None,
        None,
        None,
        None,
        Some(transcript_path),
        None,
        published_branch,
    )
    .await
}

/// Mount a resumed conversation inside a background worker's real PTY.
/// Reuses the standard resume construction and replay path while threading the
/// background registration and its one-shot initial prompt into the normal TUI
/// mount.
pub(crate) async fn mount_background_resumed_tui(
    argv: &Argv,
    session_id: uuid::Uuid,
    messages: Vec<JsonlMessage>,
    registration: std::sync::Arc<crate::agents_registry::SessionRegistration>,
    initial_prompt: Option<Utf16JsonProjection>,
    handoff: Option<lingxi_core::host::BackgroundingSnapshot>,
    shell_launch: &crate::background_launch::BackgroundLaunchSpec,
) -> crate::mode::RunOutcome {
    let published_branch = match shell_launch.published_source.as_ref() {
        Some(receipt) => match receipt.recover(
            &lingxi_home_dir(),
            session_id,
            std::path::Path::new(&shell_launch.transcript_path),
        ) {
            Ok(source) => Some(source),
            Err(error) => {
                eprintln!("lingxi-cli: published background branch changed: {error}");
                return crate::mode::RunOutcome::Exit(exit_codes::RUNTIME_ERROR);
            }
        },
        None => None,
    };
    mount_resumed_tui_inner(
        argv,
        session_id,
        messages,
        None,
        Some(registration),
        initial_prompt,
        handoff,
        Some(shell_launch),
        Some(PathBuf::from(&shell_launch.transcript_path)),
        None,
        published_branch,
    )
    .await
}

pub(super) async fn mount_resumed_tui_inner(
    argv: &Argv,
    session_id: uuid::Uuid,
    messages: Vec<JsonlMessage>,
    carried_state: Option<crate::mode::RemountState>,
    registration: Option<std::sync::Arc<crate::agents_registry::SessionRegistration>>,
    initial_prompt: Option<Utf16JsonProjection>,
    handoff: Option<lingxi_core::host::BackgroundingSnapshot>,
    shell_launch: Option<&crate::background_launch::BackgroundLaunchSpec>,
    transcript_path: Option<PathBuf>,
    admitted_remount_mode: Option<lingxi_core::host::session_context::ConversationMode>,
    published_branch: Option<crate::init::PublishedBranchSource>,
) -> crate::mode::RunOutcome {
    let resume_cwd = if let Some(source) = published_branch.as_ref() {
        match crate::init::selected_resume_workspace(source) {
            Ok(cwd) => cwd,
            Err(error) => {
                eprintln!("lingxi-cli: resume workspace failed: {error}");
                return crate::mode::RunOutcome::Exit(exit_codes::RUNTIME_ERROR);
            }
        }
    } else if let Some(launch) = shell_launch {
        PathBuf::from(&launch.cwd)
    } else if let Some(state) = carried_state.as_ref() {
        state.cwd.clone()
    } else {
        eprintln!("lingxi-cli: resume workspace is unavailable");
        return crate::mode::RunOutcome::Exit(exit_codes::RUNTIME_ERROR);
    };
    if !resume_cwd.is_absolute() || !resume_cwd.is_dir() {
        eprintln!("lingxi-cli: resume workspace is not an existing absolute directory");
        return crate::mode::RunOutcome::Exit(exit_codes::RUNTIME_ERROR);
    }
    if let Some(launch) = shell_launch {
        if std::path::Path::new(&launch.cwd) != resume_cwd {
            eprintln!("lingxi-cli: background resume workspace changed after approval");
            return crate::mode::RunOutcome::Exit(exit_codes::RUNTIME_ERROR);
        }
    } else {
        let (mode, notice) =
            crate::permission_settings::resolve_permission_mode_at(argv, &resume_cwd);
        if let Err(error) =
            crate::permission_settings::enforce_permission_mode_safety(argv, mode).await
        {
            eprintln!("{error}");
            return crate::mode::RunOutcome::Exit(exit_codes::RUNTIME_ERROR);
        }
        if let Some(notice) = notice {
            eprintln!("{notice}");
        }
        if !crate::mode::startup_preflight_at(argv, &resume_cwd).await {
            return crate::mode::RunOutcome::Exit(exit_codes::RUNTIME_ERROR);
        }
    }
    // Build derives inherited effort from the writer-claimed captured source,
    // after validating the selected path and before provider assembly.
    let resumed_argv = argv.clone();
    let explicit_permission_mode = resume_has_permission_mode_override(argv);
    // Build with the RESUMED session id as the JSONL writer's file name, so new
    // turns append to `<session_id>.jsonl` (the loaded file) instead of forking a
    // fresh-uuid file — the fix for resume splitting a conversation across files.
    let parent_session_id = parent_session_id_from_messages(&messages);
    let native_settings_agent_context =
        native_settings_agent_context_for_launch(shell_launch.map(|launch| launch.launch));
    let selected_transcript = transcript_path.clone();
    let mut tui_build =
        match crate::init::build_runtime_for_tui_inner_with_parent_and_native_settings_context(
            &resumed_argv,
            Some(session_id),
            parent_session_id,
            native_settings_agent_context,
            transcript_path,
            admitted_remount_mode.or_else(|| {
                (!shell_launch.is_some_and(|launch| launch.options.preserve_admitted_context))
                    .then_some(lingxi_core::host::session_context::ConversationMode::Code)
            }),
            shell_launch.map(|launch| launch.session_profile.into()),
            published_branch,
            resume_cwd,
        )
        .await
        {
            Ok(b) => b,
            Err(e) => {
                eprintln!("lingxi-cli: tui init failed: {e}");
                return crate::mode::RunOutcome::Exit(exit_codes::RUNTIME_ERROR);
            }
        };
    if let Some(launch) = shell_launch {
        if let Err(error) =
            crate::background_launch::acknowledge_published_source(&lingxi_home_dir(), launch)
        {
            eprintln!("lingxi-cli: could not acknowledge background branch mount: {error}");
            return crate::mode::RunOutcome::Exit(exit_codes::RUNTIME_ERROR);
        }
    }
    let entries = if let Some(captured) = tui_build.captured_resume.take() {
        // Assembly mounted this exact claimed revision before startup hooks;
        // reuse it for the UI and deferred replay without overwriting hook rows.
        captured.entries
    } else {
        // A caller without a captured boot source uses the in-hand messages.
        let effective_permission_mode = match seed_orchestrator_session(
            &tui_build.runtime.orchestrator,
            session_id,
            &messages,
            tui_build.initial_permission_mode,
            explicit_permission_mode,
            resume_has_model_override(argv, tui_build.runtime.model_provenance),
        )
        .await
        {
            Ok(mode) => mode,
            Err(error) => {
                eprintln!("lingxi-cli: resume permission mode failed: {error}");
                return crate::mode::RunOutcome::Exit(exit_codes::RUNTIME_ERROR);
            }
        };
        tui_build.initial_permission_mode = effective_permission_mode;
        let entries = match selected_transcript.as_deref() {
            Some(path) => load_resume_entries(session_id, path).await,
            None => Err(LoaderError::Io {
                arg: session_id.to_string(),
                source: std::io::Error::other("selected transcript path is unavailable"),
            }),
        };
        let entries = match entries {
            Ok(entries) => entries,
            Err(error) => {
                eprintln!("lingxi-cli: resume deferred tools failed: {error}");
                return crate::mode::RunOutcome::Exit(exit_codes::RUNTIME_ERROR);
            }
        };
        tui_build
            .runtime
            .orchestrator
            .restore_resume_prompt_metadata(&entries)
            .await;
        entries
    };
    let deferred_tools = orchestrator::deferred_tool_replays_from_messages(&entries);
    // COST seed on resume: the durable ledger owns this. Every host now has a
    // coordinator, and `SessionStateManager`'s captured opening-balance import
    // seeds the same `lastCost` figure into the WAL exactly once per session,
    // before the tracker is published. Reading it a second time here would add
    // the prior total on top of a projection that already contains it.
    // LIVE-STATE carry (parity with claude-code's in-place `/rewind`, a React
    // setState that never rebuilds and so keeps model + fast-mode + plan-mode):
    // apply the state the OUTGOING runtime had, carried IN MEMORY through the
    // `RunOutcome`, so the rebuilt runtime keeps the user's mid-session toggles
    // rather than the boot/config defaults. `None` on a cold `--resume`. All are
    // pure session-state writes, run BEFORE `run_ratatui` — model drives the
    // `session.models` the freshly-read status snapshot builds (display follows);
    // fast/plan are read live by the turn loop, no cached display to seed.
    let mut boot_notice = None;
    let mut draft_store = None;
    if let Some(state) = carried_state {
        draft_store = Some(state.draft_store.clone());
        let orch = &tui_build.runtime.orchestrator;
        if let Some((model, profile)) = state.model {
            let _ = orch.switch_model(&model, profile.as_deref()).await;
        }
        if let Some(user_model_setting) = state.user_model_setting {
            let session = orch.session();
            let mut session = session.lock().await;
            // Carry the complete source/effective-source/route binding from
            // the outgoing runtime; rebuilding it from the visible model would
            // lose unresolved source identity and its routing anchor.
            session.user_model_setting = Some(user_model_setting);
            session.user_model_setting_provenance = lingxi_core::host::ModelProvenance::UserOrEnv;
            session.user_model_setting_source =
                lingxi_core::session::SessionModelSettingSource::UserSelection;
        }
        let _ = orch.set_fast_mode(state.fast_mode).await;
        if state.permission_mode.is_none() {
            let _ = orch.set_plan_mode(state.plan_mode).await;
        }
        // Restore the live permission mode the user was in (Shift+Tab): apply it
        // to BOTH the freshly-built enforcing gate (so tool checks follow it) and
        // the indicator seed (`initial_permission_mode` drives the bottom-of-
        // composer badge). Without this the re-mount reset the mode to the
        // CLI/config default even though the process never restarted.
        if let Some(wire) = state.permission_mode {
            if let Err(error) = orch.set_permission_mode(&wire).await {
                tracing::warn!(%error, "could not restore the live permission mode");
            }
            if let Some(mode) = orch.permission_mode() {
                tui_build.initial_permission_mode =
                    permission::permission_mode_from_cli_string(&mode);
            }
        }
        boot_notice = state.notice;
    }
    if let Some(launch) = shell_launch {
        if let Err(error) = crate::shell_handoff::restore_destination(
            &daemon_runtime_dir(),
            &launch.short,
            tui_build.runtime.task_registry.as_ref(),
            &launch.shell_handoff,
        )
        .await
        {
            eprintln!("lingxi-cli: shell handoff restore failed: {error}");
            return crate::mode::RunOutcome::Exit(exit_codes::RUNTIME_ERROR);
        }
    }
    // RENDER seed: use the complete routed transcript for scrollback, while
    // `messages` above remains the compacted resumable chain used to seed the
    // engine. This keeps pre-compaction rows visible without putting them back
    // into the model context. Cold `--resume` has no SessionRegistration (fresh
    // launches register). In-process `/resume` remounts pass the live
    // registration through so status + permissionClass stay on `sessions/<pid>.json`.
    // A carried one-shot notice (the `/branch` success confirmation) renders as
    // the newest system cell.
    let mut resumed_messages = tui::replay::rebuild_from_jsonl_with_request_ids(&entries);
    if let Some(body) = boot_notice {
        resumed_messages.push(tui_core::message::RenderedMessage::SystemText {
            body,
            timestamp: 0,
            is_error: false,
        });
    }
    crate::mode::run_ratatui_with_initial_state(
        tui_build,
        registration,
        resumed_messages,
        initial_prompt,
        handoff,
        draft_store,
        deferred_tools,
    )
    .await
}

/// Recover the source session stamped by `session::branch::create_branch`.
/// Legacy and ordinary transcripts have no marker and intentionally return
/// `None`.
pub(crate) fn parent_session_id_from_messages(messages: &[JsonlMessage]) -> Option<String> {
    messages.iter().find_map(|entry| {
        entry
            .extra
            .string_units("/forkedFrom/sessionId")
            .and_then(|units| String::from_utf16(&units).ok())
            .filter(|id| !id.is_empty())
    })
}

/// Drive a mounted TUI, following any in-session `/resume` switch by re-mounting
/// the chosen session in-process until the user quits.
///
/// This is the seam that makes `/resume` a REAL mid-conversation switch WITHOUT
/// an in-place `resume_session` swap (which does not retarget the JSONL writer,
/// so it would fork the conversation across files). Each switch:
///   1. admits navigation only when the outgoing session is idle, then tears
///      down that runtime before this loop sees `RunOutcome::SwitchTo`; then
///   2. rebuilds a fresh runtime pinned to the target session file via the SAME
///      proven startup resume seam ([`mount_resumed_tui`] →
///      `build_runtime_for_tui_inner(argv, Some(id))`), replaying its scrollback.
///
/// A load failure for the switch TARGET must NOT kill the process: the session
/// that was just driving the loop is a known-good, resumable file (it was live a
/// moment ago), so a failed `/resume` re-mounts THAT session instead of exiting
/// (finding #2). Only if even the fallback re-mount fails — a genuinely
/// unrecoverable state — does the loop surface the error and exit.
///
/// `initial_session_id` is the id of the session that produced `first` (both
/// mount sites know it); it seeds the fallback so even a FIRST failed switch has
/// a session to return to.
pub(crate) async fn drive_tui_switch_loop(
    argv: &Argv,
    first: crate::mode::RunOutcome,
    initial_session_id: Option<uuid::Uuid>,
    registration: Option<std::sync::Arc<crate::agents_registry::SessionRegistration>>,
) -> i32 {
    drive_tui_switch_loop_inner(argv, first, initial_session_id, registration, None).await
}

/// Background variant that preserves the worker registration across every
/// `/resume`, `/branch`, and `/rewind` remount.
pub(crate) async fn drive_background_tui_switch_loop(
    argv: &Argv,
    first: crate::mode::RunOutcome,
    initial_session_id: Option<uuid::Uuid>,
    registration: std::sync::Arc<crate::agents_registry::SessionRegistration>,
    preserve_admitted_context: bool,
) -> i32 {
    let admitted_session = preserve_admitted_context
        .then_some(initial_session_id)
        .flatten();
    drive_tui_switch_loop_inner(
        argv,
        first,
        initial_session_id,
        Some(registration),
        admitted_session,
    )
    .await
}

pub(super) async fn remount_tui(
    argv: &Argv,
    session_id: uuid::Uuid,
    messages: Vec<JsonlMessage>,
    transcript_path: PathBuf,
    captured: Option<crate::init::PublishedBranchSource>,
    state: Option<crate::mode::RemountState>,
    registration: Option<std::sync::Arc<crate::agents_registry::SessionRegistration>>,
    admitted_session: Option<uuid::Uuid>,
) -> crate::mode::RunOutcome {
    remount_tui_with_branch(
        argv,
        session_id,
        messages,
        transcript_path,
        state,
        registration,
        admitted_session,
        captured,
    )
    .await
}

async fn remount_tui_with_branch(
    argv: &Argv,
    session_id: uuid::Uuid,
    messages: Vec<JsonlMessage>,
    transcript_path: PathBuf,
    state: Option<crate::mode::RemountState>,
    registration: Option<std::sync::Arc<crate::agents_registry::SessionRegistration>>,
    admitted_session: Option<uuid::Uuid>,
    published_branch: Option<crate::init::PublishedBranchSource>,
) -> crate::mode::RunOutcome {
    let admitted_mode = if admitted_session == Some(session_id) {
        state.as_ref().map(|state| state.conversation_mode)
    } else {
        None
    };
    mount_resumed_tui_inner(
        argv,
        session_id,
        messages,
        state,
        registration,
        None,
        None,
        None,
        Some(transcript_path),
        admitted_mode,
        published_branch,
    )
    .await
}

struct RemountSource {
    path: PathBuf,
    messages: Vec<JsonlMessage>,
    captured: crate::init::PublishedBranchSource,
}

/// A picker selection cannot be replaced while the old runtime shuts down.
fn retain_picker_source(
    mut source: RemountSource,
    captured: Option<crate::init::PublishedBranchSource>,
) -> Result<RemountSource, LoaderError> {
    if let Some(captured) = captured {
        let current = captured
            .authority
            .read_checked()
            .map_err(|error| LoaderError::Io {
                arg: source.path.display().to_string(),
                source: std::io::Error::other(error.to_string()),
            })?;
        if captured.authority.path() != source.path
            || !captured.authority.same_source(&source.captured.authority)
            || captured.content != source.captured.content
            || current != captured.content.as_bytes()
        {
            return Err(LoaderError::Io {
                arg: source.path.display().to_string(),
                source: std::io::Error::other("selected transcript changed; select it again"),
            });
        }
        source.captured = captured;
    }
    Ok(source)
}

/// Retain the mounted source for the same UUID; only a different session may
/// be resolved from the worktree catalog. Both paths receive the same exact
/// source validation as the actual runtime capture.
async fn remount_source_from(
    home: &std::path::Path,
    cwd: &std::path::Path,
    current: Option<uuid::Uuid>,
    state: Option<&crate::mode::RemountState>,
    target: uuid::Uuid,
    admitted_session: Option<uuid::Uuid>,
) -> Result<RemountSource, LoaderError> {
    remount_selected_source_from(home, cwd, current, state, target, admitted_session, None).await
}

async fn remount_selected_source_from(
    home: &std::path::Path,
    cwd: &std::path::Path,
    current: Option<uuid::Uuid>,
    state: Option<&crate::mode::RemountState>,
    target: uuid::Uuid,
    admitted_session: Option<uuid::Uuid>,
    selected_path: Option<&std::path::Path>,
) -> Result<RemountSource, LoaderError> {
    let path = if current == Some(target) {
        let retained = state
            .and_then(|state| state.transcript_path.clone())
            .ok_or_else(|| LoaderError::Io {
                arg: target.to_string(),
                source: std::io::Error::other("mounted transcript path is unavailable"),
            })?;
        if let Some(selected) = selected_path {
            let selected = crate::init::validate_exact_tui_resume_path(home, target, selected)
                .map_err(|error| LoaderError::Io {
                    arg: target.to_string(),
                    source: std::io::Error::other(error.to_string()),
                })?;
            let retained_canonical = crate::init::validate_exact_tui_resume_path(
                home, target, &retained,
            )
            .map_err(|error| LoaderError::Io {
                arg: target.to_string(),
                source: std::io::Error::other(error.to_string()),
            })?;
            if selected != retained_canonical {
                return Err(LoaderError::Io {
                    arg: target.to_string(),
                    source: std::io::Error::other(
                        "session is already mounted from another transcript",
                    ),
                });
            }
        }
        retained
    } else if let Some(path) = selected_path {
        path.to_path_buf()
    } else {
        crate::init::resolve_unique_cli_resume_path(home, target).map_err(|error| {
            LoaderError::Io {
                arg: target.to_string(),
                source: std::io::Error::other(error.to_string()),
            }
        })?
    };
    let expected_mode = if current == Some(target) && admitted_session == Some(target) {
        state
            .map(|state| state.conversation_mode)
            .ok_or_else(|| LoaderError::Io {
                arg: target.to_string(),
                source: std::io::Error::other("admitted session mode is unavailable"),
            })?
    } else {
        lingxi_core::host::session_context::ConversationMode::Code
    };
    let source = remount_source_at(home, cwd, target, &path, expected_mode).await?;
    if current == Some(target) {
        let authority = state
            .and_then(|state| state.transcript_authority.as_ref())
            .ok_or_else(|| LoaderError::Io {
                arg: target.to_string(),
                source: std::io::Error::other("mounted transcript authority is unavailable"),
            })?;
        if !authority.same_source(&source.captured.authority)
            || authority.read_checked().map_err(|error| LoaderError::Io {
                arg: target.to_string(),
                source: std::io::Error::other(error.to_string()),
            })? != source.captured.content.as_bytes()
        {
            return Err(LoaderError::Io {
                arg: target.to_string(),
                source: std::io::Error::other("mounted transcript changed; select it again"),
            });
        }
    }
    Ok(source)
}

async fn remount_source_at(
    home: &std::path::Path,
    _cwd: &std::path::Path,
    target: uuid::Uuid,
    path: &std::path::Path,
    expected_mode: lingxi_core::host::session_context::ConversationMode,
) -> Result<RemountSource, LoaderError> {
    let captured =
        crate::init::capture_selected_cli_source(home, target, path, Some(expected_mode)).map_err(
            |error| LoaderError::Io {
                arg: path.display().to_string(),
                source: std::io::Error::other(error.to_string()),
            },
        )?;
    let path = captured.authority.path();
    let loaded =
        session::jsonl::loader::route_captured_session_source(&path, target, &captured.content)?;
    let (messages, _) = session::jsonl::build_conversation_chain(&loaded, &target.to_string());
    if messages.is_empty()
        && !crate::init::is_valid_anchored_empty_source(&path, target, &captured.content, &loaded)
    {
        return Err(LoaderError::EmptyDirectory);
    }
    Ok(RemountSource {
        path,
        messages,
        captured,
    })
}

async fn remount_source(
    current: Option<uuid::Uuid>,
    state: Option<&crate::mode::RemountState>,
    target: uuid::Uuid,
    admitted_session: Option<uuid::Uuid>,
    selected_path: Option<&std::path::Path>,
) -> Result<RemountSource, LoaderError> {
    let home = lingxi_home_dir();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    remount_selected_source_from(
        &home,
        &cwd,
        current,
        state,
        target,
        admitted_session,
        selected_path,
    )
    .await
}

/// Source mutations run under the same UUID writer claim as a mounted
/// persistent session, and revalidate the path after acquiring it.
fn claim_mounted_transcript(
    home: &std::path::Path,
    session_id: uuid::Uuid,
    state: Option<&crate::mode::RemountState>,
) -> Result<(PathBuf, lingxi_core::host::live_sessions::SessionIdClaim), String> {
    let state = state.ok_or_else(|| "mounted session state is unavailable".to_string())?;
    if !state.transcript_persistent {
        return Err("mounted session is read-only".into());
    }
    let path = state
        .transcript_path
        .as_ref()
        .ok_or_else(|| "mounted transcript path is unavailable".to_string())?;
    let claim = lingxi_core::host::live_sessions::LiveSessionDir::at_live(home.join("sessions"))
        .claim_session_id(&session_id.to_string(), std::process::id())
        .map_err(|error| format!("session writer claim failed: {error}"))?;
    let path = crate::init::validate_exact_tui_resume_path(home, session_id, path)
        .map_err(|error| error.to_string())?;
    let authority = state
        .transcript_authority
        .as_ref()
        .ok_or_else(|| "mounted transcript authority is unavailable".to_string())?;
    if authority.path() != path {
        return Err("mounted transcript source changed".into());
    }
    authority
        .read_checked()
        .map_err(|error| error.to_string())?;
    Ok((path, claim))
}

/// Rewind only the captured mounted source. A successful rewrite returns its
/// published identity and exact bytes for the next mount; failures never
/// recapture a replacement at the same path.
async fn rewind_mounted_source(
    home: &std::path::Path,
    cwd: &std::path::Path,
    session_id: uuid::Uuid,
    state: Option<&crate::mode::RemountState>,
    target: uuid::Uuid,
    scope: tui::bottom_pane::view::RewindScope,
) -> Result<crate::init::PublishedBranchSource, String> {
    use tui::bottom_pane::view::RewindScope;
    if !matches!(
        scope,
        RewindScope::CodeOnly | RewindScope::ConversationOnly | RewindScope::CodeAndConversation
    ) {
        return Err("summarize is not a rewind operation".into());
    }
    let (path, claim) = claim_mounted_transcript(home, session_id, state)?;
    let state = state.ok_or("mounted session state is unavailable")?;
    let authority = state
        .transcript_authority
        .as_ref()
        .ok_or("mounted transcript authority is unavailable")?
        .clone();
    let content = String::from_utf8(
        authority
            .read_checked()
            .map_err(|error| error.to_string())?,
    )
    .map_err(|_| "mounted transcript is not UTF-8")?;
    authority
        .verify_current()
        .map_err(|error| error.to_string())?;
    if authority.path() != path {
        return Err("mounted transcript source changed".into());
    }
    let lease = claim.into_shared();
    if scope != RewindScope::ConversationOnly {
        let cwd_text = cwd.to_str().ok_or("rewind workspace is not UTF-8")?;
        let outcome = session::file_history::rewind_from_captured(
            home, cwd_text, session_id, state.conversation_mode, &authority, &content, target,
            lease.clone(),
        )
        .await
        .map_err(|error| format!(
            "file rewind failed; workspace may be partially restored, transcript not rewritten: {error}"
        ))?;
        if outcome.failed > 0 || outcome.skipped_links > 0 {
            return Err(format!(
                "file rewind incomplete ({} restored, {} failed, {} unsafe); workspace may be partially restored, transcript not rewritten",
                outcome.changed.len(), outcome.failed, outcome.skipped_links
            ));
        }
        eprintln!("lingxi-cli: rewound {} file(s)", outcome.changed.len());
    }
    if scope != RewindScope::CodeOnly {
        let published = session::rewind::rewind_conversation(
            &authority,
            &content,
            session_id,
            state.conversation_mode,
            cwd,
            target,
            lease,
        )
        .await
        .map_err(|error| {
            if scope == RewindScope::CodeAndConversation {
                format!("workspace restore completed, but conversation rewind failed: {error}")
            } else {
                format!("conversation rewind failed: {error}")
            }
        })?;
        Ok(crate::init::PublishedBranchSource {
            authority: published.authority,
            content: published.content,
        })
    } else {
        if authority
            .read_checked()
            .map_err(|error| error.to_string())?
            != content.as_bytes()
        {
            return Err("rewind source changed; workspace may be partially restored".into());
        }
        authority
            .verify_current()
            .map_err(|error| error.to_string())?;
        Ok(crate::init::PublishedBranchSource { authority, content })
    }
}

/// Branch the selected persistent mount, revalidating it while the shared
/// source claim is held by `create_independent_cli_branch_from_source`.
async fn branch_mounted_cli(
    home: &std::path::Path,
    cwd: &str,
    source: uuid::Uuid,
    state: Option<&crate::mode::RemountState>,
    title: Option<&str>,
) -> Result<session::branch::PublishedBranch, String> {
    let state = state.ok_or_else(|| "mounted session state is unavailable".to_string())?;
    if !state.transcript_persistent {
        return Err("mounted session is read-only".into());
    }
    let path = state
        .transcript_path
        .as_deref()
        .ok_or_else(|| "mounted transcript path is unavailable".to_string())?;
    let captured =
        crate::init::capture_selected_cli_source(home, source, path, Some(state.conversation_mode))
            .map_err(|error| error.to_string())?;
    let authority = state
        .transcript_authority
        .as_ref()
        .ok_or_else(|| "mounted transcript authority is unavailable".to_string())?;
    if !authority.same_source(&captured.authority) {
        return Err("mounted transcript source changed".into());
    }
    create_independent_cli_branch_from_source(
        home,
        cwd,
        source,
        uuid::Uuid::new_v4(),
        Some(path),
        title,
        Some(&captured),
    )
    .await
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AgentOpenPlan {
    StayOnCurrent,
    RemountTarget,
    QueueBackgroundResume {
        short: String,
        session_id: Option<String>,
    },
}

pub(super) fn plan_agent_open(
    target: &tui::bottom_pane::view::AgentSessionTarget,
    disposition: &crate::commands::attach::AttachDisposition,
) -> AgentOpenPlan {
    use crate::commands::attach::AttachDisposition;
    match disposition {
        AttachDisposition::Attached | AttachDisposition::LiveEndpointUnavailable { .. } => {
            AgentOpenPlan::StayOnCurrent
        }
        AttachDisposition::NotFound if target.live => AgentOpenPlan::StayOnCurrent,
        AttachDisposition::NotFound => AgentOpenPlan::RemountTarget,
        AttachDisposition::NotRunning { short, session_id } if target.background => {
            AgentOpenPlan::QueueBackgroundResume {
                short: short.clone(),
                session_id: session_id.clone(),
            }
        }
        AttachDisposition::NotRunning { .. } if target.live => AgentOpenPlan::StayOnCurrent,
        AttachDisposition::NotRunning { .. } => AgentOpenPlan::RemountTarget,
    }
}

pub(super) fn set_agent_open_notice(
    state: &mut Option<crate::mode::RemountState>,
    message: impl Into<String>,
) {
    let message = message.into();
    if let Some(state) = state.as_mut() {
        state.notice = Some(message.clone());
    }
    eprintln!("lingxi-cli: {message}");
}

pub(super) async fn drive_tui_switch_loop_inner(
    argv: &Argv,
    first: crate::mode::RunOutcome,
    initial_session_id: Option<uuid::Uuid>,
    registration: Option<std::sync::Arc<crate::agents_registry::SessionRegistration>>,
    admitted_session: Option<uuid::Uuid>,
) -> i32 {
    struct InboxShutdown;
    impl Drop for InboxShutdown {
        fn drop(&mut self) {
            if let Err(error) = lingxi_core::host::uds_inbox::stop_process_inbox_checked() {
                eprintln!("lingxi-cli: cross-session inbox drain failed: {error}");
            }
        }
    }
    let _inbox = InboxShutdown;
    let mut outcome = first;
    // The session currently driving the loop — the fallback for a failed switch.
    let mut current = initial_session_id;
    loop {
        match outcome {
            crate::mode::RunOutcome::Exit(code) => return code,
            crate::mode::RunOutcome::OpenAgentSession { target, mut state } => {
                let home = lingxi_home_dir();
                let selector = target.session_id.to_string();
                let disposition = match crate::commands::attach::attach_target(&home, &selector) {
                    Ok(disposition) => disposition,
                    Err(error) => {
                        set_agent_open_notice(
                            &mut state,
                            format!("couldn't attach to agent {selector}: {error}"),
                        );
                        let Some(current) = current else {
                            return exit_codes::RUNTIME_ERROR;
                        };
                        outcome = crate::mode::RunOutcome::SwitchTo {
                            target: current,
                            source_path: None,
                            source_receipt: None,
                            state,
                        };
                        continue;
                    }
                };
                let plan = plan_agent_open(&target, &disposition);
                let mount_target = match plan {
                    AgentOpenPlan::RemountTarget => target.session_id,
                    AgentOpenPlan::StayOnCurrent => {
                        if !matches!(
                            disposition,
                            crate::commands::attach::AttachDisposition::Attached
                        ) {
                            set_agent_open_notice(
                                &mut state,
                                format!(
                                    "agent {selector} is still owned by another process; its transcript was not reopened"
                                ),
                            );
                        }
                        let Some(current) = current else {
                            return exit_codes::RUNTIME_ERROR;
                        };
                        current
                    }
                    AgentOpenPlan::QueueBackgroundResume { short, session_id } => {
                        let resolved_session = session_id.as_deref().unwrap_or(&selector);
                        match crate::commands::respawn::queue_resume_for_short_if_safe(
                            &home,
                            &short,
                            resolved_session,
                        ) {
                            Ok(true) => {
                                crate::background_dispatch::ensure_daemon_for_control(&home);
                                match crate::commands::agents::wait_for_auto_resumed_attach(
                                    &home,
                                    resolved_session,
                                ) {
                                    Ok(crate::commands::agents::OpenSessionDisposition::Attached) => {}
                                    Ok(
                                        crate::commands::agents::OpenSessionDisposition::LiveEndpointUnavailable {
                                            ..
                                        }
                                        | crate::commands::agents::OpenSessionDisposition::NotRunning {
                                            ..
                                        },
                                    ) => set_agent_open_notice(
                                        &mut state,
                                        format!(
                                            "agent {selector} is restarting in the background; try opening it again in a moment"
                                        ),
                                    ),
                                    Ok(crate::commands::agents::OpenSessionDisposition::ForegroundResume) => {
                                        set_agent_open_notice(
                                            &mut state,
                                            format!(
                                                "agent {selector} changed while its restart was being queued"
                                            ),
                                        );
                                    }
                                    Err(error) => set_agent_open_notice(
                                        &mut state,
                                        format!("couldn't attach to agent {selector}: {error}"),
                                    ),
                                }
                                let Some(current) = current else {
                                    return exit_codes::RUNTIME_ERROR;
                                };
                                current
                            }
                            // `false` can mean either a terminal row OR that a
                            // concurrent daemon/delete/restart won the exact
                            // state CAS. Only the former proves the transcript
                            // may be mounted as a foreground writer.
                            Ok(false) => {
                                let safely_terminal = crate::agents_registry::read_job(
                                    &home, &short,
                                )
                                .is_some_and(|job| {
                                    crate::agents_registry::job_is_terminal(&job)
                                        && job.phase.as_deref()
                                            != Some(crate::commands::respawn::PHASE_DELETING)
                                        && job.worker_pid.is_none()
                                        && job.worker_proc_start.is_none()
                                });
                                if safely_terminal {
                                    target.session_id
                                } else {
                                    set_agent_open_notice(
                                        &mut state,
                                        format!(
                                            "agent {selector} changed while its restart was being queued"
                                        ),
                                    );
                                    let Some(current) = current else {
                                        return exit_codes::RUNTIME_ERROR;
                                    };
                                    current
                                }
                            }
                            Err(error) => {
                                set_agent_open_notice(
                                    &mut state,
                                    format!("couldn't restart agent {selector}: {error}"),
                                );
                                let Some(current) = current else {
                                    return exit_codes::RUNTIME_ERROR;
                                };
                                current
                            }
                        }
                    }
                };
                outcome = crate::mode::RunOutcome::SwitchTo {
                    target: mount_target,
                    source_path: None,
                    source_receipt: None,
                    state,
                };
            }
            crate::mode::RunOutcome::SwitchTo {
                target,
                source_path,
                source_receipt,
                state,
            } => {
                let selected = remount_source(
                    current,
                    state.as_ref(),
                    target,
                    admitted_session,
                    source_path.as_deref(),
                )
                .await
                .and_then(|source| retain_picker_source(source, source_receipt));
                // Reject an untrusted target while the outgoing session still
                // remains the fallback for this switch.
                let selected = match selected {
                    Ok(source) if current != Some(target) => {
                        match crate::init::selected_resume_workspace(&source.captured) {
                            Ok(cwd) if crate::mode::startup_preflight_at(argv, &cwd).await => {
                                Ok(source)
                            }
                            Ok(_) => Err(LoaderError::Io {
                                arg: target.to_string(),
                                source: std::io::Error::other("workspace approval declined"),
                            }),
                            Err(error) => Err(LoaderError::Io {
                                arg: target.to_string(),
                                source: std::io::Error::other(error.to_string()),
                            }),
                        }
                    }
                    other => other,
                };
                match selected {
                    Ok(source) => {
                        current = Some(target);
                        outcome = remount_tui(
                            argv,
                            target,
                            source.messages,
                            source.path,
                            Some(source.captured),
                            state,
                            registration.clone(),
                            admitted_session,
                        )
                        .await;
                    }
                    Err(e) => {
                        // The outgoing runtime is already unwound, so we cannot just
                        // continue it — but we CAN re-mount the session it was, which
                        // reloads cleanly. Never exit on a single failed switch when a
                        // working session was in progress.
                        eprintln!("lingxi-cli: couldn't resume {target}: {e}");
                        match recover_from_failed_switch(current) {
                            SwitchRecovery::Remount(fallback) => {
                                eprintln!("lingxi-cli: staying in current session {fallback}");
                                match remount_source(
                                    current,
                                    state.as_ref(),
                                    fallback,
                                    admitted_session,
                                    None,
                                )
                                .await
                                {
                                    Ok(source) => {
                                        outcome = remount_tui(
                                            argv,
                                            fallback,
                                            source.messages,
                                            source.path,
                                            Some(source.captured),
                                            state,
                                            registration.clone(),
                                            admitted_session,
                                        )
                                        .await;
                                    }
                                    Err(e2) => {
                                        // Double failure: even the known-good session
                                        // won't reload. Genuinely unrecoverable.
                                        eprintln!(
                                            "lingxi-cli: failed to re-mount current session \
                                         {fallback}: {e2}"
                                        );
                                        return exit_codes::RUNTIME_ERROR;
                                    }
                                }
                            }
                            SwitchRecovery::Exit(code) => return code,
                        }
                    }
                }
            }
            crate::mode::RunOutcome::BranchFrom { title, state } => {
                let Some(source) = current else {
                    eprintln!("lingxi-cli: cannot branch — no active session");
                    return exit_codes::RUNTIME_ERROR;
                };
                let lingxi_home = lingxi_home_dir();
                let cwd = state
                    .as_ref()
                    .map(|state| state.cwd.clone())
                    .unwrap_or_else(|| {
                        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                    });
                let cwd_str = cwd.to_string_lossy().into_owned();
                let mut state = state;
                let branch = branch_mounted_cli(
                    &lingxi_home,
                    &cwd_str,
                    source,
                    state.as_ref(),
                    title.as_deref(),
                )
                .await;
                let (mount_target, published_branch) = match branch {
                    Ok(published) => {
                        let result = &published.result;
                        // claude-code 2.1.205 success confirmation, rendered in
                        // the NEW branch's transcript: `Branched
                        // conversation (Branch N). You are now in the new
                        // branch (session <new>). Use /resume <src> to return
                        // to the original, or run `lingxi-cli -r <src>` in a
                        // new terminal.` The saved title already embeds the
                        // " (Branch[ N])" marker — reuse it.
                        let marker = result
                            .title
                            .rfind(" (Branch")
                            .map(|i| result.title[i..].to_string())
                            .unwrap_or_default();
                        let notice = format!(
                            "Branched conversation{marker}. You are now in the new branch \
                             (session {new}). Use /resume {src} to return to the original, \
                             or run `lingxi-cli -r {src}` in a new terminal.",
                            new = result.new_session_id,
                            src = result.source_session_id,
                        );
                        if let Some(s) = state.as_mut() {
                            s.notice = Some(notice);
                        }
                        (result.new_session_id, Some(published))
                    }
                    Err(e) if e == "No conversation to branch" => {
                        // claude-code's in-transcript empty-state line (was a
                        // stderr-only eprintln): re-mount the source session
                        // carrying the notice.
                        if let Some(s) = state.as_mut() {
                            s.notice = Some("No conversation to branch".to_string());
                        }
                        (source, None)
                    }
                    Err(e) => {
                        // Branch creation failed; never drop the user —
                        // re-mount the still-good source session.
                        eprintln!("lingxi-cli: failed to branch conversation: {e}");
                        (source, None)
                    }
                };
                let selected = if mount_target == source {
                    remount_source_from(
                        &lingxi_home,
                        &cwd,
                        current,
                        state.as_ref(),
                        source,
                        admitted_session,
                    )
                    .await
                } else {
                    match published_branch.as_ref() {
                        Some(published) => {
                            match remount_source_at(
                                &lingxi_home,
                                &cwd,
                                mount_target,
                                &published.authority.path(),
                                lingxi_core::host::session_context::ConversationMode::Code,
                            )
                            .await
                            {
                                Ok(selected)
                                    if selected
                                        .captured
                                        .authority
                                        .same_source(&published.authority)
                                        && selected.captured.content == published.content =>
                                {
                                    Ok(selected)
                                }
                                Ok(_) => Err(LoaderError::Io {
                                    arg: mount_target.to_string(),
                                    source: std::io::Error::other(
                                        "published branch changed before mount",
                                    ),
                                }),
                                Err(error) => Err(error),
                            }
                        }
                        None => Err(LoaderError::Io {
                            arg: mount_target.to_string(),
                            source: std::io::Error::other("branch transcript path is unavailable"),
                        }),
                    }
                };
                match selected {
                    Ok(selected) => {
                        current = Some(mount_target);
                        outcome = remount_tui_with_branch(
                            argv,
                            mount_target,
                            selected.messages,
                            selected.path,
                            state,
                            registration.clone(),
                            admitted_session,
                            published_branch.map(Into::into).or(Some(selected.captured)),
                        )
                        .await;
                    }
                    Err(e) => {
                        // The branch (or fallback) target won't load. Fall back to
                        // the known-good source, mirroring the SwitchTo recovery.
                        eprintln!("lingxi-cli: couldn't open {mount_target}: {e}");
                        match recover_from_failed_switch(current) {
                            SwitchRecovery::Remount(fallback) => {
                                match remount_source(
                                    current,
                                    state.as_ref(),
                                    fallback,
                                    admitted_session,
                                    None,
                                )
                                .await
                                {
                                    Ok(selected) => {
                                        outcome = remount_tui(
                                            argv,
                                            fallback,
                                            selected.messages,
                                            selected.path,
                                            Some(selected.captured),
                                            state,
                                            registration.clone(),
                                            admitted_session,
                                        )
                                        .await;
                                    }
                                    Err(e2) => {
                                        eprintln!(
                                            "lingxi-cli: failed to re-mount current session \
                                             {fallback}: {e2}"
                                        );
                                        return exit_codes::RUNTIME_ERROR;
                                    }
                                }
                            }
                            SwitchRecovery::Exit(code) => return code,
                        }
                    }
                }
            }
            crate::mode::RunOutcome::RewindTo {
                message,
                scope,
                mut state,
            } => {
                let Some(source) = current else {
                    eprintln!("lingxi-cli: cannot rewind — no active session");
                    return exit_codes::RUNTIME_ERROR;
                };
                let lingxi_home = lingxi_home_dir();
                let cwd = state
                    .as_ref()
                    .map(|state| state.cwd.clone())
                    .unwrap_or_else(|| {
                        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                    });
                let captured = match rewind_mounted_source(
                    &lingxi_home,
                    &cwd,
                    source,
                    state.as_ref(),
                    message,
                    scope,
                )
                .await
                {
                    Ok(captured) => captured,
                    Err(error) => {
                        eprintln!("lingxi-cli: cannot rewind: {error}");
                        return exit_codes::RUNTIME_ERROR;
                    }
                };
                let path = captured.authority.path();
                let messages = match load_resume_snapshot(source, &captured) {
                    Ok(messages) => messages,
                    Err(error) => {
                        eprintln!("lingxi-cli: cannot reopen rewound snapshot: {error}");
                        return exit_codes::RUNTIME_ERROR;
                    }
                };
                if let Some(state) = state.as_mut() {
                    state.transcript_path = Some(path.clone());
                    state.transcript_authority = Some(captured.authority.clone());
                }
                current = Some(source);
                outcome = remount_tui(
                    argv,
                    source,
                    messages,
                    path,
                    Some(captured),
                    state,
                    registration.clone(),
                    admitted_session,
                )
                .await;
            }
        }
    }
}

/// What to do after a `/resume` switch target fails to load, given the session
/// that was driving the loop. Pure so the "never exit while a session is in
/// progress" contract (finding #2) is unit-testable without mounting a TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SwitchRecovery {
    /// Re-mount this known-good session instead of exiting.
    Remount(uuid::Uuid),
    /// No session to fall back to (defensive — both mount sites seed one); exit.
    Exit(i32),
}

pub(super) fn recover_from_failed_switch(current: Option<uuid::Uuid>) -> SwitchRecovery {
    match current {
        Some(id) => SwitchRecovery::Remount(id),
        None => SwitchRecovery::Exit(exit_codes::RUNTIME_ERROR),
    }
}

/// Seed an already-built orchestrator's in-memory [`lingxi_core::SessionState`] from
/// a resumed transcript.
///
/// The fresh-mount path builds the orchestrator via `harness_runtime::desktop::build`,
/// which hands back an `Arc<ConversationOrchestrator>` with a fresh, empty
/// session — it has no resume parameter. Rather than introduce a second,
/// divergent resumed-orchestrator construction path, we rebuild the
/// `SessionState` from the transcript lines ALREADY in hand via the
/// orchestrator's own public replay mapping
/// ([`orchestrator::state_from_messages`], the same per-line conversion its
/// `with_resume` constructor uses — no redundant disk re-read, no TOCTOU window),
/// then overwrite the live session through its public `session()` accessor (an
/// `Arc<Mutex<SessionState>>`). We copy `history` + `session_id` so the resumed
/// id is reported and a follow-up turn appends onto the prior history.
pub(crate) async fn seed_orchestrator_session(
    orchestrator: &Arc<orchestrator::ConversationOrchestrator>,
    session_id: uuid::Uuid,
    messages: &[JsonlMessage],
    resolved_permission_mode: permission::PermissionMode,
    explicit_cli_permission_mode: bool,
    preserve_boot_model: bool,
) -> Result<permission::PermissionMode, String> {
    let replayed = orchestrator::state_from_messages(session_id, messages);
    let effective_permission_mode = effective_resume_permission_mode(
        resolved_permission_mode,
        explicit_cli_permission_mode,
        replayed.plan_mode,
    );
    orchestrator
        .set_permission_mode(effective_permission_mode.wire_str())
        .await?;
    // Seed the parent-uuid chain off the resumed transcript's tail so the first
    // append after resume chains cleanly (the writer file is the same
    // `<session_id>.jsonl` when built via `build_runtime_for_tui_inner`).
    let last_uuid = messages
        .iter()
        .rev()
        .map(|m| m.uuid.clone())
        .find(|u| !u.is_empty());
    orchestrator.seed_last_jsonl_uuid(last_uuid).await;
    // `session()` returns an owned `Arc<Mutex<SessionState>>`; bind it so the
    // lock guard does not borrow a temporary that is freed at end-of-statement.
    let session_handle = orchestrator.session();
    let mut session = session_handle.lock().await;
    session.session_id = replayed.session_id;
    session.history = replayed.history;
    session.compacted_user_turns = replayed.compacted_user_turns;
    session.virtual_user_messages = replayed.virtual_user_messages;
    session.transcript_only_messages = replayed.transcript_only_messages;
    session.compact_summary_messages = replayed.compact_summary_messages;
    session.model_context_excluded_messages = replayed.model_context_excluded_messages;
    session.active_goal = replayed.active_goal;
    // A picker choice is durable before the next assistant response exists.
    // Explicit/configured boot selections therefore win over stale transcript
    // model metadata, including its provider routing profile. With no configured
    // selection the old conversation retains its own model.
    if !preserve_boot_model {
        session.model = replayed.model;
        session.model_profile = replayed.model_profile;
    }
    session.plan_mode = effective_permission_mode == permission::PermissionMode::Plan;
    if session.plan_mode {
        session.plan_reminder_shown = false;
    }
    drop(session);
    orchestrator
        .sync_active_goal_stop_hook_for_current_state()
        .await;
    orchestrator.restore_resume_runtime_metadata(messages).await;
    Ok(effective_permission_mode)
}

pub(super) fn resume_has_model_override(
    argv: &Argv,
    provenance: lingxi_core::host::ModelProvenance,
) -> bool {
    argv.model.is_some()
        || argv.agent.is_some()
        || provenance != lingxi_core::host::ModelProvenance::ProviderCatalogTier
}

pub(super) fn resume_has_permission_mode_override(argv: &Argv) -> bool {
    argv.permission_mode.is_some()
        || argv.dangerously_skip_permissions
        || crate::permission_mode_preference::load(argv).is_some()
}

pub(super) fn effective_resume_permission_mode(
    resolved_permission_mode: permission::PermissionMode,
    explicit_cli_permission_mode: bool,
    replayed_plan_mode: bool,
) -> permission::PermissionMode {
    if explicit_cli_permission_mode {
        resolved_permission_mode
    } else if replayed_plan_mode {
        permission::PermissionMode::Plan
    } else {
        resolved_permission_mode
    }
}

/// The line picker retains the selected row, including its physical receipt,
/// before returning control to an async caller.
async fn select_exact_session_interactive<R, W>(
    rows: &[SessionMetadata],
    home: &std::path::Path,
    stdin: &mut tokio::io::BufReader<R>,
    stdout: &mut W,
) -> Result<Option<(uuid::Uuid, PathBuf, crate::init::PublishedBranchSource)>, LoaderError>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

    if rows.is_empty() {
        return Err(LoaderError::EmptyDirectory);
    }
    let mut out = String::from("Resume which session?\n");
    for (index, row) in rows.iter().enumerate() {
        let modified = session::jsonl::loader::format_rfc3339_seconds(row.modified);
        out.push_str(&format!("  {}. {} [{}]\n", index + 1, row.title, modified));
    }
    stdout
        .write_all(out.as_bytes())
        .await
        .map_err(|source| LoaderError::Io {
            arg: "stdout".into(),
            source,
        })?;
    for _ in 0..3 {
        stdout
            .write_all(b"> ")
            .await
            .map_err(|source| LoaderError::Io {
                arg: "stdout".into(),
                source,
            })?;
        stdout.flush().await.map_err(|source| LoaderError::Io {
            arg: "stdout".into(),
            source,
        })?;
        let mut line = String::new();
        let n = stdin
            .read_line(&mut line)
            .await
            .map_err(|source| LoaderError::Io {
                arg: "stdin".into(),
                source,
            })?;
        if n == 0 || line.trim().is_empty() {
            return Ok(None);
        }
        match line
            .trim()
            .parse::<usize>()
            .ok()
            .and_then(|index| index.checked_sub(1))
            .and_then(|index| rows.get(index))
        {
            Some(row) => {
                let captured = crate::init::capture_selected_cli_source(
                    home,
                    row.uuid,
                    &row.path,
                    Some(lingxi_core::host::session_context::ConversationMode::Code),
                )
                .map_err(|error| LoaderError::Io {
                    arg: row.path.display().to_string(),
                    source: std::io::Error::other(error.to_string()),
                })?;
                return Ok(Some((row.uuid, row.path.clone(), captured)));
            }
            None => {
                let message = format!(
                    "Please enter a number from 1 to {}, or empty to cancel.\n",
                    rows.len()
                );
                stdout
                    .write_all(message.as_bytes())
                    .await
                    .map_err(|source| LoaderError::Io {
                        arg: "stdout".into(),
                        source,
                    })?;
            }
        }
    }
    Err(LoaderError::InvalidSelection)
}

/// `--resume` (no id) under `--no-tui` / non-TTY — the line-based picker
/// over sessions in the current cwd's project dir.
///
/// On `Ok(Some(uuid))` surface "Resumed session {uuid}"; on `Ok(None)`
/// (cancel / EOF) print "Cancelled."; on `Err(EmptyDirectory)` print the
/// M5-08 "No conversations found to resume." All return [`exit_codes::SUCCESS`]
/// except a hard I/O failure (`RUNTIME_ERROR`).
pub(super) async fn run_resume_stdio_picker(argv: &Argv, sink: &dyn OutputSink) -> i32 {
    use tokio::io::BufReader;

    let rows = match load_resume_rows().await {
        // (M4 cc2.1.198) `--from-pr` narrows the picker rows (`filterByPr`);
        // a no-flag `--resume` passes through unchanged.
        Ok(rows) => filter_rows_by_pr(rows, argv.from_pr.as_deref()),
        Err(LoaderError::EmptyDirectory) => {
            sink.text("No conversations found to resume.\n").await;
            return exit_codes::SUCCESS;
        }
        Err(e) => {
            sink.error("runtime", &e.to_string()).await;
            return exit_codes::RUNTIME_ERROR;
        }
    };
    if rows.is_empty() {
        // A PR filter over rows with no PR metadata yields the same locked
        // empty-state line as an empty project dir.
        sink.text("No conversations found to resume.\n").await;
        return exit_codes::SUCCESS;
    }

    let mut stdin = BufReader::new(tokio::io::stdin());
    let mut stdout = tokio::io::stdout();
    match select_exact_session_interactive(&rows, &lingxi_home_dir(), &mut stdin, &mut stdout).await
    {
        Ok(Some((uuid, path, captured))) => {
            let (target, path, published) =
                match initial_resume_target(argv, uuid, path, Some(captured)).await {
                    Ok(target) => target,
                    Err(error) => {
                        sink.error("runtime", &error).await;
                        return exit_codes::RUNTIME_ERROR;
                    }
                };
            sink.text(&format!("Resumed session {target}\n")).await;
            crate::repl::run_repl_resumed_with_stdin(argv, target, path, published, stdin).await
        }
        Ok(None) => {
            sink.text("Cancelled.\n").await;
            exit_codes::SUCCESS
        }
        Err(e) => {
            sink.error("runtime", &e.to_string()).await;
            exit_codes::RUNTIME_ERROR
        }
    }
}

/// `--resume` (no id) under a full TTY — open the ratatui Resume picker over
/// the same M5-08 loader rows. A selected UUID is mounted through the standard
/// resumed-TUI path; cancelling exits without constructing a runtime.
pub(super) async fn run_resume_iocraft(argv: &Argv, sink: &dyn OutputSink) -> i32 {
    let rows = match load_resume_rows().await {
        // (M4 cc2.1.198) `--from-pr` narrows the picker rows (`filterByPr`);
        // a no-flag `--resume` passes through unchanged. An emptied list
        // renders the same empty-state Resume screen as an empty project dir.
        Ok(rows) => filter_rows_by_pr(rows, argv.from_pr.as_deref()),
        Err(LoaderError::EmptyDirectory) => {
            // Render the empty-state screen so the user still sees the locked
            // "No conversations found to resume." line, then cancels out.
            Vec::new()
        }
        Err(e) => {
            sink.error("runtime", &e.to_string()).await;
            return exit_codes::RUNTIME_ERROR;
        }
    };

    // Map the loader metadata into the picker's lean rows (the picker crate does
    // not depend on the `session` loader).
    let picker_rows = map_resume_rows(&rows);

    // Blocking terminal IO → off the async runtime, like the chat `run_app`.
    let home = lingxi_home_dir();
    let picked = tokio::task::spawn_blocking(move || {
        tui::resume::run_resume_picker(picker_rows, |selection| {
            crate::init::capture_selected_cli_source(
                &home,
                selection.uuid,
                &selection.transcript_path,
                Some(lingxi_core::host::session_context::ConversationMode::Code),
            )
            .map_err(std::io::Error::other)
        })
    })
    .await;

    match picked {
        Ok(Ok(Some((selection, captured)))) => {
            let (uuid, path, published) = match initial_resume_target(
                argv,
                selection.uuid,
                selection.transcript_path,
                Some(captured),
            )
            .await
            {
                Ok(target) => target,
                Err(error) => {
                    sink.error("runtime", &error).await;
                    return exit_codes::RUNTIME_ERROR;
                }
            };
            let messages = match load_resume_snapshot(
                uuid,
                published.as_ref().expect("resume source was captured"),
            ) {
                Ok(messages) => messages,
                Err(error) => {
                    sink.error("runtime", &error.to_string()).await;
                    return exit_codes::RUNTIME_ERROR;
                }
            };
            let first = mount_resumed_tui(argv, uuid, messages, path, None, published).await;
            drive_tui_switch_loop(argv, first, Some(uuid), None).await
        }
        // Claude Code 2.1.321's startup picker cancels silently with status 1.
        // This never constructs a runtime or changes the saved transcript.
        Ok(Ok(None)) => exit_codes::RUNTIME_ERROR,
        Ok(Err(e)) => {
            sink.error("runtime", &e.to_string()).await;
            exit_codes::RUNTIME_ERROR
        }
        Err(e) => {
            sink.error("runtime", &e.to_string()).await;
            exit_codes::RUNTIME_ERROR
        }
    }
}

/// Load the recent-session rows for the current cwd via the M5-08 loader.
/// Shared by the stdio + iocraft branches (DRY). Resolves `lingxi_home`
/// (`$LINGXI_CONFIG_DIR` → `~/.claude`), the cwd, and the native disk-backed
/// host filesystem — the same loader inputs M5-08 expects, then delegates
/// to the pure [`load_resume_rows_from`].
pub(crate) async fn load_resume_rows() -> Result<Vec<SessionMetadata>, LoaderError> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    load_resume_rows_for_cwd(&cwd).await
}

/// In-session callers must use the live session cwd, not the process cwd.
pub(crate) async fn load_resume_rows_for_cwd(
    cwd: &std::path::Path,
) -> Result<Vec<SessionMetadata>, LoaderError> {
    let lingxi_home = lingxi_home_dir();
    let mut rows = load_resume_rows_from(&lingxi_home, cwd).await?;
    for row in &mut rows {
        row.path =
            crate::init::absolute_resolved_tui_resume_path(row.path.clone()).map_err(|error| {
                LoaderError::Io {
                    arg: row.uuid.to_string(),
                    source: std::io::Error::other(error.to_string()),
                }
            })?;
    }
    Ok(rows)
}

/// Production disk→[`SessionMetadata`] path with the inputs passed in (no env /
/// process-cwd reads), so it is directly testable. Builds the same disk-backed
/// native host filesystem the live branches use and
/// asks the M5-08 loader for the complete sorted catalog. The picker itself
/// exposes this in 50-row pages (`tui::resume::RESUME_PAGE_SIZE`), matching
/// Claude Code 2.1.246's `allStatLogs` / `nextIndex` behavior.
pub(crate) async fn load_resume_rows_from(
    lingxi_home: &std::path::Path,
    cwd: &std::path::Path,
) -> Result<Vec<SessionMetadata>, LoaderError> {
    let cwd_str = cwd.to_string_lossy().into_owned();
    let fs: Arc<dyn FileSystem> = Arc::new(HostFileSystem::new(cwd.to_path_buf()));
    session::jsonl::list_recent_sessions_in_scope_with_mode(
        lingxi_home,
        &cwd_str,
        session::jsonl::SessionDiscoveryScope::Current,
        usize::MAX,
        fs,
        Some(session::jsonl::SessionMode::Code),
    )
    .await
    .map(|catalog| catalog.sessions)
}

/// Map M5-08 loader metadata into the picker's lean [`tui::resume::ResumeRow`]s
/// (newest-first). The dim metadata line is built with the picker's
/// `relative_time_ago` so it stays byte-identical to the alt-screen picker:
/// `<relative time ago> · <N> messages`. Shared by the startup `--resume` picker
/// ([`run_resume_iocraft`]) and the in-session `/resume` preload.
pub(crate) fn map_resume_rows(rows: &[SessionMetadata]) -> Vec<tui::resume::ResumeRow> {
    let now = std::time::SystemTime::now();
    rows.iter()
        .map(|m| {
            let msgs = if m.message_count == 1 {
                "1 message".to_string()
            } else {
                format!("{} messages", m.message_count)
            };
            tui::resume::ResumeRow {
                uuid: m.uuid,
                transcript_path: m.path.clone(),
                title: m.title.clone(),
                metadata_label: format!(
                    "{} \u{00b7} {}",
                    tui::resume::relative_time_ago(m.modified, now),
                    msgs
                ),
            }
        })
        .collect()
}

/// Read the in-session picker catalog for the captured live working directory.
/// The async host calls this at mount and after `/cd`, outside the ratatui loop.
/// Returns an empty list on any load error (the picker then shows its
/// empty-state screen), so a scan failure never blocks the mount.
pub(crate) async fn load_resume_catalog_for_cwd(cwd: &std::path::Path) -> Vec<SessionMetadata> {
    load_resume_rows_for_cwd(cwd).await.unwrap_or_default()
}

/// Reload deferred transcript entries from the already selected source.
pub(super) async fn load_resume_entries(
    session_id: uuid::Uuid,
    path: &std::path::Path,
) -> Result<Vec<JsonlMessage>, LoaderError> {
    let lingxi_home = lingxi_home_dir();
    let path = crate::init::validate_exact_tui_resume_path(&lingxi_home, session_id, path)
        .map_err(|error| LoaderError::Io {
            arg: path.display().to_string(),
            source: std::io::Error::other(error.to_string()),
        })?;
    let (content, _) =
        crate::init::read_pinned_cli_resume_source(&lingxi_home, &path).map_err(|error| {
            LoaderError::Io {
                arg: path.display().to_string(),
                source: std::io::Error::other(error.to_string()),
            }
        })?;
    let loaded =
        session::jsonl::loader::route_captured_session_source(&path, session_id, &content)?;
    Ok(loaded.messages_in_order)
}

/// Exercise bare-UUID resolution with explicit filesystem roots in tests.
#[cfg(test)]
pub(crate) async fn load_resume_session_from(
    lingxi_home: &std::path::Path,
    cwd: &std::path::Path,
    session_id: uuid::Uuid,
) -> Result<Vec<JsonlMessage>, LoaderError> {
    let path =
        crate::init::resolve_unique_cli_resume_path(lingxi_home, session_id).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                LoaderError::SessionNotFound {
                    arg: session_id.to_string(),
                }
            } else {
                LoaderError::Io {
                    arg: session_id.to_string(),
                    source: error,
                }
            }
        })?;
    load_resume_session_at(lingxi_home, cwd, session_id, &path).await
}

async fn load_resume_session_at(
    lingxi_home: &std::path::Path,
    _cwd: &std::path::Path,
    session_id: uuid::Uuid,
    path: &std::path::Path,
) -> Result<Vec<JsonlMessage>, LoaderError> {
    let captured = crate::init::capture_selected_cli_source(
        lingxi_home,
        session_id,
        path,
        Some(lingxi_core::host::session_context::ConversationMode::Code),
    )
    .map_err(|error| LoaderError::Io {
        arg: path.display().to_string(),
        source: std::io::Error::other(error.to_string()),
    })?;
    load_resume_snapshot(session_id, &captured)
}

fn load_resume_snapshot(
    session_id: uuid::Uuid,
    captured: &crate::init::PublishedBranchSource,
) -> Result<Vec<JsonlMessage>, LoaderError> {
    let loaded = session::jsonl::loader::route_captured_session_source(
        &captured.authority.path(),
        session_id,
        &captured.content,
    )?;
    let (chain, _) = session::jsonl::build_conversation_chain(&loaded, &session_id.to_string());
    if chain.is_empty()
        && !crate::init::is_valid_anchored_empty_source(
            &captured.authority.path(),
            session_id,
            &captured.content,
            &loaded,
        )
    {
        return Err(LoaderError::EmptyDirectory);
    }
    Ok(chain)
}

/// Claude config home dir. `$LINGXI_CONFIG_DIR` when set wins (including an
/// empty value, meaning the invoking cwd), else `~/.claude`.
/// Shared across the CLI's settings/MCP/desktop-config resolution (`lib`, `mode`,
/// `init`) so every user-tier path honors `$LINGXI_CONFIG_DIR`.
pub(crate) fn lingxi_home_dir() -> PathBuf {
    if let Ok(explicit) = std::env::var(branding::CONFIG_DIR_ENV) {
        let path = PathBuf::from(explicit);
        return match std::env::current_dir() {
            Ok(cwd) => absolute_config_home_at(&path, &cwd),
            Err(_) => path,
        };
    }
    dirs::home_dir().map_or_else(
        || PathBuf::from(branding::DOT_DIR),
        |h| h.join(branding::DOT_DIR),
    )
}

pub(super) fn absolute_config_home_at(path: &std::path::Path, cwd: &std::path::Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

/// The background-agent daemon runtime dir — where `daemon.lock` + `roster.json`
/// live. Placed as direct siblings of `jobs/` and `sessions/` under the config
/// home so the `--bg` writer, the daemon supervisor, and the `agents` reader all
/// agree on one location (the only hard constraint — divergence would silently
/// split the lock/roster from the jobs the `agents` command reads). See the
/// daemon design's `parity_choices` note #1: `config_home` directly (not a
/// `daemon/` subdir) for simplicity.
pub(crate) fn daemon_runtime_dir() -> PathBuf {
    lingxi_home_dir()
}

/// Resolve the `--resume <ID>` argument into a concrete UUID.
pub(super) fn resolve_session_id(arg: &str) -> Result<uuid::Uuid, LoaderError> {
    // Accept both a bare `<uuid>` and the `sess:<uuid>` display form — the
    // "Session … saved. Resume with: lingxi --resume <id>" hint prints the
    // prefixed `SessionId` Display, so a user copying it verbatim must work.
    // The on-disk JSONL is named by the bare uuid, so we normalize to that.
    let body = arg.strip_prefix("sess:").unwrap_or(arg);
    uuid::Uuid::parse_str(body).map_err(|_| LoaderError::SessionNotFound {
        arg: arg.to_string(),
    })
}

/// Map a `--resume <uuid>` load outcome to the user-facing error to emit, if
/// any. `None` means the session loaded — keep the "Resumed session {id}"
/// success path. On [`LoaderError::SessionNotFound`] this returns the
/// TS-faithful "No conversation found with session ID: {id}" line
/// (claude-code/src/main.tsx:3681); any other loader failure maps to TS's
/// catch-arm "Failed to resume session {id}" (main.tsx:3704). Both carry
/// [`exit_codes::RUNTIME_ERROR`] (TS `exitWithError` → exit 1). Pure so the
/// SESSION.4 existence check is unit-testable without a `Runtime` / sink.
pub(super) fn resume_by_id_error(
    session_id: uuid::Uuid,
    loaded: Result<&Vec<JsonlMessage>, &LoaderError>,
) -> Option<(String, i32)> {
    match loaded {
        Ok(_) => None,
        Err(LoaderError::SessionNotFound { .. }) => Some((
            format!("No conversation found with session ID: {session_id}"),
            exit_codes::RUNTIME_ERROR,
        )),
        Err(LoaderError::Io { source, .. })
            if source.to_string().contains("session_mode_mismatch") =>
        {
            Some((source.to_string(), exit_codes::RUNTIME_ERROR))
        }
        Err(_) => Some((
            format!("Failed to resume session {session_id}"),
            exit_codes::RUNTIME_ERROR,
        )),
    }
}

fn native_settings_agent_context_for_launch(
    launch: Option<crate::background_launch::BackgroundLaunchKind>,
) -> lingxi_core::host::NativeSettingsAgentContext {
    if launch == Some(crate::background_launch::BackgroundLaunchKind::Fork) {
        lingxi_core::host::NativeSettingsAgentContext::Subagent {
            is_main_session: true,
        }
    } else {
        lingxi_core::host::NativeSettingsAgentContext::Main
    }
}

#[cfg(test)]
mod native_settings_agent_context_tests {
    use super::*;
    use crate::background_launch::BackgroundLaunchKind as Launch;
    use lingxi_core::host::NativeSettingsAgentContext as Context;

    #[test]
    fn only_an_explicit_background_fork_uses_main_session_subagent_identity() {
        assert_eq!(
            native_settings_agent_context_for_launch(None),
            Context::Main
        );
        assert_eq!(
            native_settings_agent_context_for_launch(Some(Launch::Fresh)),
            Context::Main
        );
        assert_eq!(
            native_settings_agent_context_for_launch(Some(Launch::Resume)),
            Context::Main
        );
        assert_eq!(
            native_settings_agent_context_for_launch(Some(Launch::Fork)),
            Context::Subagent {
                is_main_session: true
            }
        );
    }
}

#[cfg(test)]
#[path = "exact_remount_path_tests.rs"]
mod exact_remount_path_tests;
