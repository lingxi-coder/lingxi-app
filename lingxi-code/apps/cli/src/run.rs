//! One-shot conversation: feed the prompt, run the orchestrator, print
//! results, exit. Also hosts the resume entrypoint.
//!
//! (M7-12) `--resume` is a three-way split routed by the pure [`resume_route`]:
//!   - `--resume <uuid>`            → [`run_resume_by_id`] (load by id).
//!   - `--resume` (no id) + TTY     → [`run_resume_iocraft`] (the iocraft
//!     Resume screen over the M5-08 loader).
//!   - `--resume` (no id) + non-TTY → [`run_resume_stdio_picker`] (the
//!     unchanged M5-08 `select_session_interactive` stdio fallback).

use crate::argv::Argv;
use crate::exit_codes;
use crate::init::Runtime;
use crate::output::OutputSink;
use session::jsonl::loader::{
    list_recent_sessions, load_session, select_session_interactive, LoaderError, SessionMetadata,
};
use session::jsonl::JsonlMessage;
use std::path::PathBuf;
use std::sync::Arc;
use traits::{FileSystem, SlashCommandDispatcher, SlashDispatchResult};

/// Drive a one-shot conversation: either a `/slash-command` or a normal
/// prompt that runs through the orchestrator turn loop.
pub async fn run_oneshot(argv: &Argv, runtime: &Runtime, sink: &dyn OutputSink) -> i32 {
    let prompt = argv.prompt.clone().unwrap_or_default();
    if prompt.trim().is_empty() {
        eprintln!("lingxi-cli: empty prompt");
        return exit_codes::ARGV_ERROR;
    }

    // Slash branch — bypasses the API entirely.
    if prompt.starts_with('/') {
        return run_slash_command(&prompt, runtime, sink).await;
    }

    // Non-slash branch — drive the orchestrator turn loop. Without a real
    // ANTHROPIC_API_KEY this returns 401; we surface the error verbatim.
    sink.turn_start().await;
    match runtime.orchestrator.run_turn(&prompt).await {
        Ok(_outcome) => exit_codes::SUCCESS,
        Err(e) => {
            sink.error("runtime", &e.to_string()).await;
            exit_codes::RUNTIME_ERROR
        }
    }
}

/// Dispatch a `/command [args]` line through the registry.
pub async fn run_slash_command(input: &str, runtime: &Runtime, sink: &dyn OutputSink) -> i32 {
    match runtime.dispatcher.dispatch(input).await {
        SlashDispatchResult::Handled { display } => {
            sink.command_output("", &display).await;
            exit_codes::SUCCESS
        }
        SlashDispatchResult::Unknown { name: _, display } => {
            sink.command_output("", &display).await;
            exit_codes::RUNTIME_ERROR
        }
        SlashDispatchResult::NotASlashCommand => {
            // Defensive: only reached when caller violated the slash-prefix
            // contract.
            sink.error("runtime", "not a slash command (internal error)")
                .await;
            exit_codes::RUNTIME_ERROR
        }
    }
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
pub async fn run_resume(argv: &Argv, runtime: &Runtime, sink: &dyn OutputSink) -> i32 {
    match resume_route(argv, crate::mode::is_full_tty()) {
        ResumeRoute::LoadById => run_resume_by_id(argv, runtime, sink).await,
        ResumeRoute::IocraftScreen => run_resume_iocraft(argv, sink).await,
        ResumeRoute::StdioPicker => run_resume_stdio_picker(argv, sink).await,
    }
}

/// `--resume <uuid>` — the concrete-id path.
///
/// Parses the arg as a UUID, then (SESSION.4) verifies the session actually
/// exists on disk via [`load_session`] BEFORE reporting success: a valid-but-
/// unknown id now errors with the TS "No conversation found with session ID:
/// {id}" line and a non-zero exit instead of the old false "Resumed session
/// {id}". Once confirmed present it surfaces the resolved id and runs a
/// follow-up turn if a prompt is supplied; the full transcript replay into a
/// live REPL is the deferred M5-13 milestone.
async fn run_resume_by_id(argv: &Argv, runtime: &Runtime, sink: &dyn OutputSink) -> i32 {
    let arg = argv.resume.as_deref().unwrap_or("");
    let session_id = match resolve_session_id(arg) {
        Ok(id) => id,
        Err(e) => {
            sink.error("runtime", &e.to_string()).await;
            return exit_codes::RUNTIME_ERROR;
        }
    };

    // SESSION.4 parity: a `--resume <uuid>` for a session that does NOT exist
    // on disk must NOT report success. TS (claude-code/src/main.tsx:3675-3681)
    // calls `loadConversationForResume(sessionId)` and, when it yields nothing,
    // exits via `exitWithError(root, "No conversation found with session ID:
    // {sessionId}")` (exit code 1). We mirror that by loading the session up
    // front and only printing "Resumed session {id}" once it is confirmed to
    // exist and parse.
    let loaded = load_resume_session(session_id).await;
    if let Some((message, code)) = resume_by_id_error(session_id, loaded.as_ref()) {
        sink.error("runtime", &message).await;
        return code;
    }
    // Session exists and parsed. The transcript replay into the live REPL is
    // the deferred M5-13 milestone, so the loaded messages are not yet threaded
    // anywhere; keep them named to document that intent.
    let _messages = loaded.unwrap_or_default();

    sink.text(&format!("Resumed session {session_id}\n")).await;

    if let Some(p) = &argv.prompt {
        if !p.trim().is_empty() {
            return run_oneshot(argv, runtime, sink).await;
        }
    }
    eprintln!("lingxi-cli: resumed; REPL not yet wired (M5-13)");
    exit_codes::NOT_IMPLEMENTED
}

/// `--resume` (no id) under `--no-tui` / non-TTY — the UNCHANGED M5-08 stdio
/// picker (`select_session_interactive`) over the 5 most-recent sessions in
/// the current cwd's project dir. The regression-free fallback.
///
/// On `Ok(Some(uuid))` surface "Resumed session {uuid}"; on `Ok(None)`
/// (cancel / EOF) print "Cancelled."; on `Err(EmptyDirectory)` print the
/// M5-08 "No conversations found to resume." All return [`exit_codes::SUCCESS`]
/// except a hard I/O failure (`RUNTIME_ERROR`).
async fn run_resume_stdio_picker(_argv: &Argv, sink: &dyn OutputSink) -> i32 {
    use tokio::io::BufReader;

    let rows = match load_resume_rows().await {
        Ok(rows) => rows,
        Err(LoaderError::EmptyDirectory) => {
            sink.text("No conversations found to resume.\n").await;
            return exit_codes::SUCCESS;
        }
        Err(e) => {
            sink.error("runtime", &e.to_string()).await;
            return exit_codes::RUNTIME_ERROR;
        }
    };

    let mut stdin = BufReader::new(tokio::io::stdin());
    let mut stdout = tokio::io::stdout();
    match select_session_interactive(&rows, &mut stdin, &mut stdout).await {
        Ok(Some(uuid)) => {
            sink.text(&format!("Resumed session {uuid}\n")).await;
            exit_codes::SUCCESS
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

/// `--resume` (no id) under a full TTY — open the iocraft Resume screen over
/// the same M5-08 loader rows. After the TUI returns, read the chosen UUID:
/// `Some(uuid)` → "Resumed session {uuid}"; `None` → "Cancelled."
async fn run_resume_iocraft(_argv: &Argv, sink: &dyn OutputSink) -> i32 {
    let rows = match load_resume_rows().await {
        Ok(rows) => rows,
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

    match tui::session::run_resume_picker(rows).await {
        Ok(Some(uuid)) => {
            sink.text(&format!("Resumed session {uuid}\n")).await;
            exit_codes::SUCCESS
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

/// Load the recent-session rows for the current cwd via the M5-08 loader.
/// Shared by the stdio + iocraft branches (DRY). Resolves `claude_home`
/// (`$CLAUDE_CONFIG_DIR` → `~/.claude`), the cwd, and a disk-backed
/// [`PosixFileSystem`] — the same loader inputs M5-08 expects, then delegates
/// to the pure [`load_resume_rows_from`].
async fn load_resume_rows() -> Result<Vec<SessionMetadata>, LoaderError> {
    let claude_home = claude_home_dir();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    load_resume_rows_from(&claude_home, &cwd).await
}

/// Production disk→[`SessionMetadata`] path with the inputs passed in (no env /
/// process-cwd reads), so it is directly testable. Builds the same disk-backed
/// [`platform_posix_minimal::PosixFileSystem`] the live branches use and
/// asks the M5-08 loader for up to 5 most-recent rows.
async fn load_resume_rows_from(
    claude_home: &std::path::Path,
    cwd: &std::path::Path,
) -> Result<Vec<SessionMetadata>, LoaderError> {
    let cwd_str = cwd.to_string_lossy().into_owned();
    let fs: Arc<dyn FileSystem> = Arc::new(platform_posix_minimal::PosixFileSystem::new(
        cwd.to_path_buf(),
    ));
    list_recent_sessions(claude_home, &cwd_str, 5, fs).await
}

/// Load a concrete session by UUID for the `--resume <uuid>` path, using the
/// live `claude_home` (`$CLAUDE_CONFIG_DIR` → `~/.claude`) + process cwd. Thin
/// env-reading wrapper over [`load_resume_session_from`] (mirrors the
/// `load_resume_rows` / `load_resume_rows_from` split so the disk logic stays
/// testable with no env / process-cwd reads).
async fn load_resume_session(session_id: uuid::Uuid) -> Result<Vec<JsonlMessage>, LoaderError> {
    let claude_home = claude_home_dir();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    load_resume_session_from(&claude_home, &cwd, session_id).await
}

/// Production disk→`Vec<JsonlMessage>` load with the inputs passed in (no env /
/// process-cwd reads) so it is directly testable. Builds the same disk-backed
/// [`platform_posix_minimal::PosixFileSystem`] the row loader uses and asks the
/// M5-07/M5-08 [`load_session`] loader for the session, which returns
/// [`LoaderError::SessionNotFound`] when no `<uuid>.jsonl` exists under the
/// cwd's project dir.
async fn load_resume_session_from(
    claude_home: &std::path::Path,
    cwd: &std::path::Path,
    session_id: uuid::Uuid,
) -> Result<Vec<JsonlMessage>, LoaderError> {
    let cwd_str = cwd.to_string_lossy().into_owned();
    let fs: Arc<dyn FileSystem> = Arc::new(platform_posix_minimal::PosixFileSystem::new(
        cwd.to_path_buf(),
    ));
    load_session(claude_home, &cwd_str, session_id, fs).await
}

/// Claude config home dir. `$CLAUDE_CONFIG_DIR` (when non-empty) wins, else
/// `~/.claude`. Mirrors the resolution the Doctor screen + session storage use.
fn claude_home_dir() -> PathBuf {
    if let Ok(explicit) = std::env::var("CLAUDE_CONFIG_DIR") {
        if !explicit.is_empty() {
            return PathBuf::from(explicit);
        }
    }
    dirs::home_dir().map_or_else(|| PathBuf::from(".claude"), |h| h.join(".claude"))
}

/// Resolve the `--resume <ID>` argument into a concrete UUID.
fn resolve_session_id(arg: &str) -> Result<uuid::Uuid, LoaderError> {
    uuid::Uuid::parse_str(arg).map_err(|_| LoaderError::SessionNotFound {
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
fn resume_by_id_error(
    session_id: uuid::Uuid,
    loaded: Result<&Vec<JsonlMessage>, &LoaderError>,
) -> Option<(String, i32)> {
    match loaded {
        Ok(_) => None,
        Err(LoaderError::SessionNotFound { .. }) => Some((
            format!("No conversation found with session ID: {session_id}"),
            exit_codes::RUNTIME_ERROR,
        )),
        Err(_) => Some((
            format!("Failed to resume session {session_id}"),
            exit_codes::RUNTIME_ERROR,
        )),
    }
}

#[cfg(test)]
mod tests {
    //! Loader-fixture coverage for the `--resume` disk→[`SessionMetadata`]→
    //! row production path (`load_resume_rows_from`). Drives the *real* CLI
    //! wiring — `platform_posix_minimal::PosixFileSystem` + the M5-08
    //! `list_recent_sessions` — over a `tempfile` fixture, with no env or
    //! process-cwd reads so the test stays deterministic and parallel-safe.

    use super::*;
    use session::jsonl::project_dir_name;
    use std::time::{Duration, SystemTime};
    use uuid::Uuid;

    /// Write one valid `<uuid>.jsonl` session file (a single first-user message
    /// in the M5-07/M5-08 on-disk format) into `project_dir`, stamp its mtime,
    /// and return the uuid. `prompt` becomes the row's extracted title.
    fn write_session(project_dir: &std::path::Path, prompt: &str, mtime: SystemTime) -> Uuid {
        let uuid = Uuid::new_v4();
        let path = project_dir.join(format!("{uuid}.jsonl"));
        let line = serde_json::json!({
            "type": "user",
            "uuid": uuid.to_string(),
            "parentUuid": null,
            "sessionId": uuid.to_string(),
            "timestamp": "2026-05-25T12:00:00.000Z",
            "cwd": "/tmp/workproj",
            "version": "0.8.0",
            "isSidechain": false,
            "userType": "external",
            "message": {"role": "user", "content": prompt},
        });
        let bytes = format!("{}\n", serde_json::to_string(&line).unwrap());
        std::fs::write(&path, bytes).unwrap();
        filetime::set_file_mtime(&path, filetime::FileTime::from_system_time(mtime)).unwrap();
        uuid
    }

    /// `<claude_home>/projects/<sanitize(cwd)>/` — the dir the loader scans.
    fn make_project_dir(claude_home: &std::path::Path, cwd: &str) -> std::path::PathBuf {
        let dir = claude_home.join("projects").join(project_dir_name(cwd));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn load_resume_rows_from_returns_sorted_rows_with_titles_and_counts() {
        let temp = tempfile::TempDir::new().unwrap();
        let claude_home = temp.path().join("home");
        // `cwd` is only used as the project-dir key; it need not exist on disk.
        let cwd = std::path::PathBuf::from("/tmp/workproj");
        let cwd_str = cwd.to_string_lossy().into_owned();
        let project_dir = make_project_dir(&claude_home, &cwd_str);

        // Three sessions with staggered mtimes; "newest" has the latest mtime.
        let base = SystemTime::now();
        let _oldest = write_session(&project_dir, "oldest prompt", base);
        let _middle = write_session(
            &project_dir,
            "middle prompt",
            base + Duration::from_secs(10),
        );
        let newest = write_session(
            &project_dir,
            "newest prompt",
            base + Duration::from_secs(20),
        );

        let rows = load_resume_rows_from(&claude_home, &cwd)
            .await
            .expect("loader should produce rows");

        assert_eq!(rows.len(), 3, "all three sessions surface as rows");
        // Newest-first (mtime desc).
        assert_eq!(rows[0].uuid, newest);
        assert_eq!(rows[0].title, "newest prompt");
        assert_eq!(rows[1].title, "middle prompt");
        assert_eq!(rows[2].title, "oldest prompt");
        for w in rows.windows(2) {
            assert!(w[0].modified >= w[1].modified, "rows sorted newest-first");
        }
        // Each fixture file has exactly one JSONL line.
        for row in &rows {
            assert_eq!(row.message_count, 1, "one message per fixture session");
        }
    }

    #[tokio::test]
    async fn load_resume_rows_from_empty_project_dir_is_empty_directory() {
        let temp = tempfile::TempDir::new().unwrap();
        let claude_home = temp.path().join("home");
        let cwd = std::path::PathBuf::from("/tmp/emptyproj");
        let cwd_str = cwd.to_string_lossy().into_owned();
        // Create the project dir but write no `.jsonl` files into it.
        make_project_dir(&claude_home, &cwd_str);

        match load_resume_rows_from(&claude_home, &cwd).await {
            Err(LoaderError::EmptyDirectory) => {}
            other => panic!("expected EmptyDirectory, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn load_resume_rows_from_missing_project_dir_is_empty_directory() {
        let temp = tempfile::TempDir::new().unwrap();
        let claude_home = temp.path().join("home");
        // No projects dir at all — the loader treats NotFound as empty-state.
        let cwd = std::path::PathBuf::from("/tmp/neverproj");

        match load_resume_rows_from(&claude_home, &cwd).await {
            Err(LoaderError::EmptyDirectory) => {}
            other => panic!("expected EmptyDirectory, got {other:?}"),
        }
    }

    // ── SESSION.4: `--resume <uuid>` existence check ────────────────────────

    #[tokio::test]
    async fn resume_by_id_nonexistent_uuid_errors_with_ts_message_and_nonzero_exit() {
        // Regression: a valid-but-unknown session id used to print a false
        // "Resumed session {id}" success. It must now error with the
        // TS-faithful line (main.tsx:3681) and a non-zero exit instead.
        let temp = tempfile::TempDir::new().unwrap();
        let claude_home = temp.path().join("home");
        let cwd = std::path::PathBuf::from("/tmp/resumeproj");
        let cwd_str = cwd.to_string_lossy().into_owned();
        // Create the project dir but write NO session file for this id.
        make_project_dir(&claude_home, &cwd_str);
        let missing = Uuid::new_v4();

        let loaded = load_resume_session_from(&claude_home, &cwd, missing).await;
        assert!(
            matches!(loaded, Err(LoaderError::SessionNotFound { .. })),
            "a missing <uuid>.jsonl must surface SessionNotFound, got {loaded:?}"
        );

        let (message, code) =
            resume_by_id_error(missing, loaded.as_ref()).expect("missing session must error");
        assert_eq!(
            message,
            format!("No conversation found with session ID: {missing}"),
            "exact TS string (claude-code/src/main.tsx:3681)"
        );
        assert_eq!(code, exit_codes::RUNTIME_ERROR);
        assert_ne!(
            code,
            exit_codes::SUCCESS,
            "a non-existent id must NOT report a zero (success) exit"
        );
    }

    #[tokio::test]
    async fn resume_by_id_existing_uuid_loads_and_does_not_error() {
        // Happy path: when the <uuid>.jsonl exists the load succeeds and
        // `resume_by_id_error` returns None, so the "Resumed session {id}"
        // success line is reached.
        let temp = tempfile::TempDir::new().unwrap();
        let claude_home = temp.path().join("home");
        let cwd = std::path::PathBuf::from("/tmp/resumeproj");
        let cwd_str = cwd.to_string_lossy().into_owned();
        let project_dir = make_project_dir(&claude_home, &cwd_str);
        let id = write_session(&project_dir, "hello", SystemTime::now());

        let loaded = load_resume_session_from(&claude_home, &cwd, id).await;
        let messages = loaded.as_ref().expect("existing session must load");
        assert_eq!(messages.len(), 1, "the single fixture line is parsed");
        assert!(
            resume_by_id_error(id, loaded.as_ref()).is_none(),
            "an existing session must NOT produce an error"
        );
    }

    #[test]
    fn resume_by_id_error_maps_other_failures_to_failed_to_resume() {
        // A non-SessionNotFound loader failure mirrors TS's catch arm
        // ("Failed to resume session {id}", main.tsx:3704) with a non-zero exit.
        let id = Uuid::new_v4();
        let err = LoaderError::InvalidSelection;
        let (message, code) =
            resume_by_id_error(id, Err(&err)).expect("a loader failure must error");
        assert_eq!(message, format!("Failed to resume session {id}"));
        assert_eq!(code, exit_codes::RUNTIME_ERROR);
    }
}
