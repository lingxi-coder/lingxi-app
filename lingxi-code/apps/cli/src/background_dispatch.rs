//! `--background`/`--bg` dispatch: the durable job + launch-spec writer.
//!
//! When `--bg` is set the CLI does NOT build the in-process engine/turn. It
//! instead, purely by writing files + ensuring the daemon:
//!
//! 1. Mints a short id + session UUID.
//! 2. Writes the durable `jobs/<short>/state.json` (the primary visibility
//!    artifact — a workerless `state:"working"` row `agents --json` renders
//!    without `--all`).
//! 3. Writes owner-readable `jobs/<short>/launch.json`, which is the immutable
//!    CLI→supervisor launch context.
//! 4. ENSURES the supervisor: if no live holder owns `daemon.lock`, spawns a
//!    detached `lingxi-cli daemon` child. The daemon discovers pending jobs and
//!    is the sole writer of `roster.json`.
//! 5. Prints `<short>` and returns.
//!
//! This is NOT the in-process `registerAsyncAgent` path
//! (`engine-desktop::background_agent`) — that is unrelated to the OS daemon.
//!
//! Where exact parity is unrecoverable (there is no source/binary/strings
//! reference for the orchestration layer) the choices are grounded, not
//! byte-verified — see the daemon design `parity_choices`. In particular,
//! `rendezvousSock` names the private protocol-v2 PTY attach endpoint, and the
//! hidden PTY child registers the live `kind:"bg"` session through
//! [`SessionRegistration::register_bg`](crate::agents_registry::SessionRegistration::register_bg).

use crate::agents_registry::{self, JobStateWrite};
use crate::argv::Argv;
use crate::background_launch::{
    BackgroundLaunchKind, BackgroundLaunchOptions, BackgroundLaunchSpec, TerminalSize,
    LAUNCH_SPEC_VERSION,
};
use crate::daemon_lock::{self, LockProbe, SystemLockProbe};
use crate::exit_codes;
use std::collections::BTreeMap;
use std::io::BufRead as _;
use std::path::{Path, PathBuf};

/// Runtime context known by the live conversation when `/fork` snapshots it.
/// Existing callers can keep using [`dispatch_forked_session`] with defaults;
/// the composition root uses the context-aware variant.
#[derive(Debug, Clone, Default)]
pub struct ForkLaunchContext {
    pub model: Option<String>,
    pub system_prompt: Option<String>,
    pub options: Option<BackgroundLaunchOptions>,
    pub transcript_path: Option<String>,
    /// Mid-turn UI boundary restored by the hidden background TUI.
    pub handoff: Option<traits::BackgroundingSnapshot>,
    /// Foreground-resolved, safety-checked permission mode. When present this
    /// replaces raw CLI/settings authority in the durable launch options.
    pub resolved_permission_mode: Option<permission::PermissionMode>,
}

/// Spawns the detached daemon child. Abstracted so the writer + handoff are
/// testable without launching a real process.
pub trait DaemonSpawner {
    /// Spawn `argv` (with `argv[0]` the executable) fully detached — the CLI
    /// must return immediately.
    fn spawn(&mut self, argv: &[String]) -> std::io::Result<()>;
}

/// Production spawner: a detached `Command` with null stdio, placed in its own
/// process group (so a terminal SIGINT/SIGHUP to the CLI's foreground group is
/// not delivered to the daemon). No `unsafe` — `process_group` is a safe
/// `CommandExt` method (the crate is `#![forbid(unsafe_code)]`, so `setsid` via
/// `pre_exec` is unavailable; a fresh process group is the detachment we can get
/// safely and is sufficient for the coherent minimum).
struct RealSpawner;

impl DaemonSpawner for RealSpawner {
    fn spawn(&mut self, argv: &[String]) -> std::io::Result<()> {
        use std::process::{Command, Stdio};
        let Some((program, args)) = argv.split_first() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "empty daemon argv",
            ));
        };
        let mut cmd = Command::new(program);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const DETACHED_PROCESS: u32 = 0x0000_0008;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
        }
        // Spawn and drop the handle — we do NOT wait.
        cmd.spawn().map(|_child| ())
    }
}

/// Epoch-millis used by the durable background launch specification.
fn now_millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// The first line of the prompt, trimmed and length-capped, used as the job
/// intent/label. `None` for an empty/whitespace-only prompt.
fn intent_from_prompt(prompt: Option<&str>) -> Option<String> {
    let first = prompt?.lines().next().unwrap_or("").trim();
    if first.is_empty() {
        return None;
    }
    // Cap the seeded intent so a giant prompt doesn't bloat the roster/job row.
    const MAX: usize = 200;
    let capped: String = first.chars().take(MAX).collect();
    Some(capped)
}

fn captured_terminal_size() -> TerminalSize {
    crossterm::terminal::size()
        .ok()
        .filter(|(cols, rows)| *cols > 0 && *rows > 0)
        .map(|(cols, rows)| TerminalSize { cols, rows })
        .unwrap_or_default()
}

fn normalize_launch_options(
    mut options: BackgroundLaunchOptions,
    cwd: &str,
) -> Result<(BackgroundLaunchOptions, Option<String>), String> {
    let Some(slug) = options.worktree.as_mut() else {
        if options.tmux.is_some() {
            return Err("--tmux requires --worktree".to_string());
        }
        return Ok((options, None));
    };
    if slug.is_empty() {
        let id = uuid::Uuid::new_v4().simple().to_string();
        *slug = format!("session-{}", &id[..8]);
    }
    validate_worktree_slug(slug)?;
    let flattened = slug.replace('/', "+");
    let path = Path::new(cwd)
        .join(branding::DOT_DIR)
        .join("worktrees")
        .join(flattened)
        .display()
        .to_string();
    Ok((options, Some(path)))
}

fn managed_worktree_root_from_cwd(cwd: &str) -> Option<String> {
    for sep in ['/', '\\'] {
        let marker = format!("{sep}{}{sep}worktrees{sep}", branding::DOT_DIR);
        let Some(idx) = cwd.find(&marker) else {
            continue;
        };
        let tail = &cwd[idx + marker.len()..];
        let leaf = tail.split(sep).next()?;
        if !leaf.is_empty() && leaf != "." && leaf != ".." {
            return Some(format!("{}{}{}", &cwd[..idx], marker, leaf));
        }
    }
    None
}

fn validate_worktree_slug(slug: &str) -> Result<(), String> {
    if slug.is_empty() || slug.len() > 64 {
        return Err("invalid --worktree name: expected 1 to 64 characters".to_string());
    }
    for segment in slug.split('/') {
        let reserved = segment.to_ascii_lowercase();
        if segment.is_empty()
            || segment == "."
            || segment == ".."
            || reserved.trim_end_matches('.') == ".git"
            || !segment
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Err(format!("invalid --worktree name: {slug}"));
        }
    }
    Ok(())
}

/// `--bg` entry point (production): resolves the shared config home + daemon
/// runtime dir and dispatches with the real spawner.
pub async fn dispatch_background(
    argv: &Argv,
    resolved_permission_mode: permission::PermissionMode,
) -> i32 {
    let preflight_argv = argv_with_resolved_permission_mode(argv, resolved_permission_mode);
    if !crate::mode::startup_preflight(&preflight_argv).await {
        return exit_codes::RUNTIME_ERROR;
    }
    let config_home = crate::run::lingxi_home_dir();
    let runtime_dir = crate::run::daemon_runtime_dir();
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    // Resolve resume/continue while still in the foreground. A bare --resume
    // mounts its normal picker here; the detached child must never own setup or
    // selection UI that no terminal is attached to yet.
    let resume_source = if argv.continue_session {
        crate::run::load_resume_picker_rows()
            .await
            .first()
            .map(|row| row.uuid)
    } else if let Some(raw) = argv.resume.as_deref() {
        if raw.trim().is_empty() {
            let rows = crate::run::load_resume_picker_rows().await;
            match tokio::task::spawn_blocking(move || tui::resume::run_resume_picker(rows)).await {
                Ok(Ok(selected)) => selected,
                Ok(Err(e)) => {
                    eprintln!("lingxi-cli: resume picker failed: {e}");
                    return exit_codes::RUNTIME_ERROR;
                }
                Err(e) => {
                    eprintln!("lingxi-cli: resume picker task failed: {e}");
                    return exit_codes::RUNTIME_ERROR;
                }
            }
        } else {
            match uuid::Uuid::parse_str(raw.trim()) {
                Ok(id) => Some(id),
                Err(_) => {
                    eprintln!("Error: Invalid session ID. Must be a valid UUID.");
                    return exit_codes::RUNTIME_ERROR;
                }
            }
        }
    } else {
        None
    };

    if (argv.resume.is_some() || argv.continue_session) && resume_source.is_none() {
        eprintln!("lingxi-cli: no session selected to run in background");
        return exit_codes::RUNTIME_ERROR;
    }
    if let Some(source_id) = resume_source {
        let target_id = if argv.fork_session {
            match argv.session_id.as_deref() {
                Some(raw) => match protocol::SessionId::parse_prefixed(raw) {
                    Some(id) => id.as_uuid(),
                    None => {
                        eprintln!("Error: Invalid session ID. Must be a valid UUID.");
                        return exit_codes::RUNTIME_ERROR;
                    }
                },
                None => uuid::Uuid::new_v4(),
            }
        } else {
            source_id
        };
        let launch_transcript = match prepare_resume_transcript(
            &config_home,
            &cwd.display().to_string(),
            &source_id.to_string(),
            &target_id.to_string(),
            argv.fork_session,
        ) {
            Ok(path) => path,
            Err(e) => {
                eprintln!("lingxi-cli: could not prepare resume transcript: {e}");
                return exit_codes::RUNTIME_ERROR;
            }
        };
        let context = ForkLaunchContext {
            options: Some(BackgroundLaunchOptions::from_argv(argv)),
            transcript_path: Some(launch_transcript.display().to_string()),
            resolved_permission_mode: Some(resolved_permission_mode),
            ..ForkLaunchContext::default()
        };
        return match dispatch_resumed_session_inner(
            &config_home,
            &runtime_dir,
            &cwd.display().to_string(),
            &target_id.to_string(),
            argv.prompt.as_deref().unwrap_or(""),
            argv.fork_session,
            &context,
            &SystemLockProbe,
            &mut RealSpawner,
        ) {
            Ok(short) => {
                println!("{short}");
                exit_codes::SUCCESS
            }
            Err(e) => {
                eprintln!("lingxi-cli: could not dispatch background session: {e}");
                exit_codes::RUNTIME_ERROR
            }
        };
    }
    dispatch_background_inner(
        argv,
        &config_home,
        &runtime_dir,
        true,
        resolved_permission_mode,
        &SystemLockProbe,
        &mut RealSpawner,
    )
}

fn argv_with_resolved_permission_mode(
    argv: &Argv,
    resolved_permission_mode: permission::PermissionMode,
) -> Argv {
    let mut resolved = argv.clone();
    resolved.permission_mode = Some(resolved_permission_mode.wire_str().to_string());
    resolved.dangerously_skip_permissions = false;
    resolved
}

/// Testable core: `config_home`/`runtime_dir`, the lock-liveness probe, and the
/// daemon spawner are all injected.
fn dispatch_background_inner<LP: LockProbe, S: DaemonSpawner>(
    argv: &Argv,
    config_home: &Path,
    runtime_dir: &Path,
    preflight_approved: bool,
    resolved_permission_mode: permission::PermissionMode,
    lock_probe: &LP,
    spawner: &mut S,
) -> i32 {
    let now = now_millis();

    // 1. Mint ids.
    let short = agents_registry::mint_short_id(config_home);
    let session_id = match argv.session_id.as_deref() {
        Some(raw) => match protocol::SessionId::parse_prefixed(raw) {
            Some(id) => id.as_uuid().to_string(),
            None => {
                eprintln!("Error: Invalid session ID. Must be a valid UUID.");
                return exit_codes::RUNTIME_ERROR;
            }
        },
        None => uuid::Uuid::new_v4().to_string(),
    };

    // 2. Fields.
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let created_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let intent = intent_from_prompt(argv.prompt.as_deref());
    let terminal = captured_terminal_size();
    let transcript_path = session::jsonl::path::session_path(config_home, &cwd, &session_id)
        .display()
        .to_string();
    let mut options = BackgroundLaunchOptions::from_argv(argv);
    options.freeze_permission_mode(resolved_permission_mode);
    let (options, worktree_path) = match normalize_launch_options(options, &cwd) {
        Ok(context) => context,
        Err(e) => {
            eprintln!("lingxi-cli: {e}");
            return exit_codes::RUNTIME_ERROR;
        }
    };

    // 3. WRITE THE JOB (the primary visibility artifact).
    let respawn_flags: Vec<String> = Vec::new();
    let job = JobStateWrite {
        state: "working",
        tempo: Some("active"),
        name: None,
        session_id: Some(&session_id),
        cwd: Some(&cwd),
        origin_cwd: Some(&cwd),
        created_at: Some(&created_at),
        intent: intent.as_deref(),
        display_intent: None,
        template: Some("bg"),
        respawn_flags: &respawn_flags,
        in_flight: None,
        backend: Some("daemon"),
        // Raw prompts are private launch context, not public fleet metadata.
        initial_prompt: None,
        detail: None,
        // No live worker yet — the supervisor records the worker pid on spawn.
        worker_pid: None,
    };
    if let Err(e) = agents_registry::write_job_state(config_home, &short, &job) {
        eprintln!("lingxi-cli: could not write background job: {e}");
        return exit_codes::RUNTIME_ERROR;
    }

    let launch_spec = BackgroundLaunchSpec {
        schema_version: LAUNCH_SPEC_VERSION,
        short: short.clone(),
        created_at: now,
        preflight_approved,
        launch: BackgroundLaunchKind::Fresh,
        session_id: session_id.clone(),
        transcript_path,
        cwd: cwd.clone(),
        origin_cwd: cwd.clone(),
        worktree_path: worktree_path.clone(),
        worktree_ownership_token: worktree_path
            .as_ref()
            .map(|_| uuid::Uuid::new_v4().to_string()),
        initial_prompt: argv.prompt.clone(),
        handoff: None,
        options,
        env: launch_env(),
        terminal,
    };
    if let Err(e) = crate::background_launch::write_launch_spec(config_home, &short, &launch_spec) {
        let _ = agents_registry::update_job_state_with_detail(
            config_home,
            &short,
            "failed",
            None,
            "could not persist background launch context",
        );
        eprintln!("lingxi-cli: could not write background launch context: {e}");
        return exit_codes::RUNTIME_ERROR;
    }

    // 4. ENSURE THE DAEMON — it discovers the durable job + launch spec and is
    // the sole owner of live roster records.
    ensure_daemon(runtime_dir, lock_probe, spawner);

    // 5. Print the job id + return.
    println!("{short}");
    exit_codes::SUCCESS
}

/// Dispatch a `/fork`-to-background COPY: a background daemon that RESUMES an
/// ALREADY-SNAPSHOTTED session (its `<session_id>.jsonl` transcript was written
/// by the caller — [`crate::bg_session_forker::CliBgSessionForker`]). Unlike
/// [`dispatch_background`] (a fresh session), this mints only the short id (the
/// session id is supplied) and records a resume/fork launch spec so the worker
/// seeds its history from the copied transcript.
///
/// Returns the minted short id on success (the caller renders the live-session
/// system line from it). Production entry point: resolves nothing itself — the
/// caller passes the already-resolved `config_home` / `runtime_dir` / `cwd` so
/// the same values back the snapshot write and this dispatch.
pub fn dispatch_forked_session(
    config_home: &Path,
    runtime_dir: &Path,
    cwd: &str,
    session_id: &str,
    prompt: &str,
) -> std::io::Result<String> {
    dispatch_forked_session_with_context(
        config_home,
        runtime_dir,
        cwd,
        session_id,
        prompt,
        &ForkLaunchContext::default(),
    )
}

/// Context-aware `/fork` dispatch used by the live TUI.
pub fn dispatch_forked_session_with_context(
    config_home: &Path,
    runtime_dir: &Path,
    cwd: &str,
    session_id: &str,
    prompt: &str,
    context: &ForkLaunchContext,
) -> std::io::Result<String> {
    dispatch_resumed_session_inner(
        config_home,
        runtime_dir,
        cwd,
        session_id,
        prompt,
        true,
        context,
        &SystemLockProbe,
        &mut RealSpawner,
    )
}

/// Resume an existing session in place as a background PTY session.
pub fn dispatch_resumed_session(
    config_home: &Path,
    runtime_dir: &Path,
    cwd: &str,
    session_id: &str,
) -> std::io::Result<String> {
    dispatch_resumed_session_with_context(
        config_home,
        runtime_dir,
        cwd,
        session_id,
        &ForkLaunchContext::default(),
    )
}

/// Resume an existing session while preserving the live foreground launch
/// options and resolved permission mode.
pub fn dispatch_resumed_session_with_context(
    config_home: &Path,
    runtime_dir: &Path,
    cwd: &str,
    session_id: &str,
    context: &ForkLaunchContext,
) -> std::io::Result<String> {
    dispatch_resumed_session_inner(
        config_home,
        runtime_dir,
        cwd,
        session_id,
        "",
        false,
        context,
        &SystemLockProbe,
        &mut RealSpawner,
    )
}

/// Testable core of [`dispatch_forked_session`] — the lock-liveness probe and
/// the daemon spawner are injected.
#[cfg(test)]
fn dispatch_forked_session_inner<LP: LockProbe, S: DaemonSpawner>(
    config_home: &Path,
    runtime_dir: &Path,
    cwd: &str,
    session_id: &str,
    prompt: &str,
    lock_probe: &LP,
    spawner: &mut S,
) -> std::io::Result<String> {
    dispatch_resumed_session_inner(
        config_home,
        runtime_dir,
        cwd,
        session_id,
        prompt,
        true,
        &ForkLaunchContext::default(),
        lock_probe,
        spawner,
    )
}

#[allow(clippy::too_many_arguments)]
fn dispatch_resumed_session_inner<LP: LockProbe, S: DaemonSpawner>(
    config_home: &Path,
    runtime_dir: &Path,
    cwd: &str,
    session_id: &str,
    prompt: &str,
    fork: bool,
    context: &ForkLaunchContext,
    lock_probe: &LP,
    spawner: &mut S,
) -> std::io::Result<String> {
    let now = now_millis();

    let short = agents_registry::mint_short_id(config_home);
    let created_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let seed_prompt = (!prompt.trim().is_empty()).then(|| prompt.to_string());
    let intent = intent_from_prompt(seed_prompt.as_deref());
    let terminal = captured_terminal_size();
    let transcript = match context.transcript_path.as_deref() {
        Some(path) => validate_transcript_path(config_home, Path::new(path))?,
        None => locate_session_transcript(config_home, cwd, session_id)?,
    };
    let effective_cwd = if fork {
        cwd.to_string()
    } else {
        transcript_cwd(&transcript).unwrap_or_else(|| cwd.to_string())
    };
    let transcript_path = transcript.display().to_string();

    // Write the durable job — same shape as `dispatch_background` but with the
    // caller-supplied (already-snapshotted) session id.
    let respawn_flags: Vec<String> = Vec::new();
    let job = JobStateWrite {
        state: "working",
        tempo: Some("active"),
        name: None,
        session_id: Some(session_id),
        cwd: Some(&effective_cwd),
        origin_cwd: Some(cwd),
        created_at: Some(&created_at),
        intent: intent.as_deref(),
        display_intent: None,
        template: Some("bg"),
        respawn_flags: &respawn_flags,
        in_flight: None,
        backend: Some("daemon"),
        // Raw prompts are private launch context, not public fleet metadata.
        initial_prompt: None,
        detail: None,
        worker_pid: None,
    };
    agents_registry::write_job_state(config_home, &short, &job)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;

    // Persist the exact resume/fork context. The daemon creates the live roster
    // record only after it has spawned the PTY supervisor.
    let mut options = context.options.clone().unwrap_or_default();
    if let Some(model) = &context.model {
        options.model = Some(model.clone());
    }
    if let Some(system_prompt) = &context.system_prompt {
        options.system_prompt = Some(system_prompt.clone());
    }
    if let Some(mode) = context.resolved_permission_mode {
        options.freeze_permission_mode(mode);
    }
    let (mut options, normalized_worktree_path) = normalize_launch_options(options, &effective_cwd)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    let worktree_path = managed_worktree_root_from_cwd(&effective_cwd).or(normalized_worktree_path);
    options.clear_boot_session_options();
    let launch_spec = BackgroundLaunchSpec {
        schema_version: LAUNCH_SPEC_VERSION,
        short: short.clone(),
        created_at: now,
        // `/fork` and resume-as-background are invoked from an already-running
        // foreground TUI that completed the same startup gates.
        preflight_approved: true,
        launch: if fork {
            BackgroundLaunchKind::Fork
        } else {
            BackgroundLaunchKind::Resume
        },
        session_id: session_id.to_string(),
        transcript_path: transcript_path.clone(),
        cwd: effective_cwd.clone(),
        origin_cwd: cwd.to_string(),
        worktree_path: worktree_path.clone(),
        worktree_ownership_token: worktree_path
            .as_ref()
            .map(|_| uuid::Uuid::new_v4().to_string()),
        initial_prompt: seed_prompt.clone(),
        handoff: context.handoff.clone(),
        options,
        env: launch_env(),
        terminal,
    };
    crate::background_launch::write_launch_spec(config_home, &short, &launch_spec)?;
    ensure_daemon(runtime_dir, lock_probe, spawner);
    Ok(short)
}

/// Resolve a session UUID to its exact, regular transcript. The current cwd's
/// standard path wins; otherwise scan the direct project buckets so explicit
/// resume remains stable when invoked from another cwd/worktree.
fn locate_session_transcript(
    config_home: &Path,
    cwd: &str,
    session_id: &str,
) -> std::io::Result<PathBuf> {
    let expected = session::jsonl::path::session_path(config_home, cwd, session_id);
    if let Ok(path) = validate_transcript_path(config_home, &expected) {
        return Ok(path);
    }

    let projects = config_home.join("projects");
    let file_name = format!("{session_id}.jsonl");
    let mut matches = Vec::new();
    for entry in std::fs::read_dir(&projects)? {
        let entry = entry?;
        let metadata = std::fs::symlink_metadata(entry.path())?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            continue;
        }
        let candidate = entry.path().join(&file_name);
        if let Ok(candidate) = validate_transcript_path(config_home, &candidate) {
            matches.push(candidate);
        }
    }
    match matches.as_slice() {
        [path] => Ok(path.clone()),
        [] => Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("session transcript {session_id} was not found"),
        )),
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("session transcript {session_id} is ambiguous across project roots"),
        )),
    }
}

/// Resolve the source transcript before considering the target id. A fork
/// receives its own durable transcript copy so subsequent target-session turns
/// append to a complete history; an in-place resume keeps the exact source.
fn prepare_resume_transcript(
    config_home: &Path,
    cwd: &str,
    source_session_id: &str,
    target_session_id: &str,
    fork: bool,
) -> std::io::Result<PathBuf> {
    let source = locate_session_transcript(config_home, cwd, source_session_id)?;
    if !fork || source_session_id == target_session_id {
        return Ok(source);
    }

    let target = session::jsonl::path::session_path(config_home, cwd, target_session_id);
    let parent = target.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "fork transcript has no parent directory",
        )
    })?;
    std::fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(
        ".{target_session_id}.fork.{}.{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    let result = (|| {
        let mut input = std::fs::File::open(&source)?;
        let mut output = platform_pty::create_current_user_private_file(&tmp)?;
        std::io::copy(&mut input, &mut output)?;
        use std::io::Write as _;
        output.flush()?;
        output.sync_all()?;
        drop(output);
        // Atomic no-replace publication: hard_link fails if an explicit target
        // session id already exists, unlike Unix rename which would overwrite.
        std::fs::hard_link(&tmp, &target)?;
        let _ = std::fs::remove_file(&tmp);
        validate_transcript_path(config_home, &target)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn validate_transcript_path(config_home: &Path, path: &Path) -> std::io::Result<PathBuf> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "background transcript is not a regular file",
        ));
    }
    let projects = std::fs::canonicalize(config_home.join("projects"))?;
    let canonical = std::fs::canonicalize(path)?;
    if !canonical.starts_with(&projects) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "background transcript escapes the projects directory",
        ));
    }
    Ok(canonical)
}

/// Recover the original runtime cwd from the transcript without loading the
/// entire history. Real transcripts may start with metadata-only records, so
/// inspect a bounded prefix rather than assuming line one is a user message.
fn transcript_cwd(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut reader = std::io::BufReader::new(file);
    let mut line = String::new();
    for _ in 0..256 {
        line.clear();
        let read = reader.read_line(&mut line).ok()?;
        if read == 0 {
            break;
        }
        if line.len() > 1024 * 1024 {
            return None;
        }
        let value: serde_json::Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if let Some(cwd) = value
            .get("cwd")
            .and_then(serde_json::Value::as_str)
            .filter(|cwd| !cwd.is_empty())
        {
            return Some(cwd.to_string());
        }
    }
    None
}

/// Spawn a detached `lingxi-cli daemon` unless a live supervisor already holds
/// `daemon.lock`. A benign double-spawn is fine — `acquire_or_yield` resolves
/// the loser via `Yield`/`AlreadyOurs` (exit 0). The spawned argv carries the
/// literal `daemon` token in `argv[1..4]` so `classify_cmdline` recognises the
/// child (a HARD constraint — else a peer would steal its lock).
fn ensure_daemon<LP: LockProbe, S: DaemonSpawner>(
    runtime_dir: &Path,
    lock_probe: &LP,
    spawner: &mut S,
) {
    if let Some(holder) = daemon_lock::read_lock(runtime_dir) {
        if lock_probe.is_alive(holder.pid) && lock_probe.is_daemon_process(holder.pid) {
            // A live supervisor already owns the fleet — nothing to spawn.
            return;
        }
    }
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "lingxi-cli".to_string());
    let argv =
        crate::process_wrapper::wrap_argv(vec![exe, daemon_lock::DAEMON_SUBCOMMAND.to_string()]);
    if let Err(e) = spawner.spawn(&argv) {
        // Best-effort: a failed spawn leaves the job/launch spec durable on disk; a
        // later `--bg` or a manually-launched daemon still adopts them.
        tracing::warn!("lingxi-cli: could not spawn background daemon: {e}");
    }
}

fn launch_env() -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    // The PTY process builder clears its environment, so preserve the minimal
    // shell/locale/certificate baseline needed for a normal interactive CLI.
    for key in [
        "PATH",
        "HOME",
        "USER",
        "SHELL",
        "TERM",
        "COLORTERM",
        "TERM_PROGRAM",
        "TERM_PROGRAM_VERSION",
        "KITTY_WINDOW_ID",
        "WT_SESSION",
        "TERMINAL_EMULATOR",
        "LC_TERMINAL",
        "LC_TERMINAL_VERSION",
        "TMPDIR",
        "TMP",
        "TEMP",
        "LANG",
        "SSH_AUTH_SOCK",
        // Windows process/runtime discovery. The PTY child receives an
        // otherwise-cleared environment, so omitting these makes cmd.exe,
        // PowerShell, installed tools, and user-level config paths diverge
        // from the foreground CLI.
        "USERPROFILE",
        "USERNAME",
        "COMSPEC",
        "PATHEXT",
        "SYSTEMROOT",
        "SYSTEMDRIVE",
        "WINDIR",
        "APPDATA",
        "LOCALAPPDATA",
        "PROGRAMDATA",
        "PROGRAMFILES",
        "PROGRAMFILES(X86)",
        "PROGRAMW6432",
    ] {
        if let Ok(value) = std::env::var(key) {
            env.insert(key.to_string(), value);
        }
    }
    for (key, value) in std::env::vars() {
        if launch_env_key_allowed(&key) {
            env.insert(key, value);
        }
    }
    env.entry("TERM".to_string())
        .or_insert_with(|| "xterm-256color".to_string());

    for key in [
        "LINGXI_CODE_PROCESS_WRAPPER",
        "CLAUDE_CODE_PROCESS_WRAPPER",
        "LINGXI_HOME",
        "LINGXI_CONFIG_DIR",
        "CLAUDE_CONFIG_DIR",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_MODEL",
        "ANTHROPIC_FOUNDRY_API_KEY",
        "ANTHROPIC_FOUNDRY_AUTH_TOKEN",
        "ANTHROPIC_FOUNDRY_BASE_URL",
        "ANTHROPIC_FOUNDRY_RESOURCE",
        "ANTHROPIC_VERTEX_PROJECT_ID",
        "ANTHROPIC_VERTEX_BASE_URL",
        "ANTHROPIC_BEDROCK_BASE_URL",
        "OPENAI_API_KEY",
        "OPENAI_PERSONAL_ACCESS_TOKEN",
        "OPENAI_CHATGPT_ACCESS_TOKEN",
        "OPENAI_CHATGPT_ACCOUNT_ID",
        "OPENROUTER_API_KEY",
        "DEEPSEEK_API_KEY",
        "GEMINI_API_KEY",
        "GOOGLE_OAUTH_TOKEN",
        "AZURE_OPENAI_API_KEY",
        "GROQ_API_KEY",
        "ZHIPU_API_KEY",
        "ZAI_API_KEY",
        "AWS_PROFILE",
        "AWS_REGION",
        "AWS_DEFAULT_REGION",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "NO_PROXY",
    ] {
        if let Ok(value) = std::env::var(key) {
            env.insert(key.to_string(), value);
        }
    }
    env
}

fn launch_env_key_allowed(key: &str) -> bool {
    if matches!(
        key,
        "LINGXI_BG_SESSION"
            | "LINGXI_BG_ATTACH_SOCK"
            | "LINGXI_BG_ATTACH_AUTH"
            | "LINGXI_BG_PTY_CHILD"
            | "LINGXI_JOB_DIR"
            | "LINGXI_SESSION_KIND"
    ) {
        return false;
    }
    if matches!(
        key,
        "TERM_PROGRAM"
            | "TERM_PROGRAM_VERSION"
            | "KITTY_WINDOW_ID"
            | "WT_SESSION"
            | "TERMINAL_EMULATOR"
    ) {
        return true;
    }
    [
        "LINGXI_",
        "CLAUDE_CODE_",
        "CLAUDE_AGENT_",
        "ANTHROPIC_",
        "OPENAI_",
        "OPENROUTER_",
        "DEEPSEEK_",
        "GEMINI_",
        "GOOGLE_",
        "AZURE_",
        "GROQ_",
        "ZHIPU_",
        "ZAI_",
        "AWS_",
        "LC_",
        "SSL_CERT_",
        "BASH_",
        "MCP_",
        "TASK_",
    ]
    .iter()
    .any(|prefix| key.starts_with(prefix))
        || matches!(
            key,
            "XDG_CONFIG_HOME"
                | "NODE_EXTRA_CA_CERTS"
                | "MAX_THINKING_TOKENS"
                | "MAX_STRUCTURED_OUTPUT_RETRIES"
                | "ENABLE_TOOL_SEARCH"
                | "ENABLE_MCP_LARGE_OUTPUT_FILES"
                | "DISABLE_AUTOUPDATER"
                | "DO_NOT_TRACK"
                | "HTTP_PROXY"
                | "HTTPS_PROXY"
                | "ALL_PROXY"
                | "NO_PROXY"
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Mutex, OnceLock};

    fn tmpdir() -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut p = std::env::temp_dir();
        p.push(format!("lingxi-bgdispatch-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// Captures the spawned argv instead of launching a process.
    #[derive(Default)]
    struct CaptureSpawner {
        spawns: Vec<Vec<String>>,
    }
    impl DaemonSpawner for CaptureSpawner {
        fn spawn(&mut self, argv: &[String]) -> std::io::Result<()> {
            self.spawns.push(argv.to_vec());
            Ok(())
        }
    }

    /// Lock probe reporting a fixed set of live daemon pids.
    struct FakeLockProbe {
        live: HashMap<i32, bool>,
    }
    impl LockProbe for FakeLockProbe {
        fn is_alive(&self, pid: i32) -> bool {
            *self.live.get(&pid).unwrap_or(&false)
        }
        fn is_daemon_process(&self, pid: i32) -> bool {
            *self.live.get(&pid).unwrap_or(&false)
        }
        fn proc_start(&self, _pid: i32, _skip: bool) -> Option<String> {
            None
        }
    }

    fn bg_argv(prompt: &str) -> Argv {
        Argv {
            prompt: Some(prompt.to_string()),
            background: true,
            ..Argv::default()
        }
    }

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn writes_job_and_launch_spec_and_spawns_daemon() {
        let home = tmpdir();
        let mut spawner = CaptureSpawner::default();
        let lockp = FakeLockProbe {
            live: HashMap::new(),
        };
        let argv = bg_argv("port the daemon supervisor\nsecond line");
        let code = dispatch_background_inner(
            &argv,
            &home,
            &home,
            true,
            permission::PermissionMode::Default,
            &lockp,
            &mut spawner,
        );
        assert_eq!(code, exit_codes::SUCCESS);

        // (a) A job row + private launch spec exist for the same short id. The
        // daemon is the sole roster writer, so dispatch must not create one.
        let jobs = agents_registry::read_jobs(&agents_registry::jobs_dir(&home));
        assert_eq!(jobs.len(), 1);
        let short = jobs[0].0.clone();
        assert_eq!(jobs[0].1.state, "working");
        assert_eq!(jobs[0].1.template.as_deref(), Some("bg"));
        assert_eq!(jobs[0].1.backend.as_deref(), Some("daemon"));
        // Intent = first prompt line only.
        assert_eq!(
            jobs[0].1.intent.as_deref(),
            Some("port the daemon supervisor")
        );

        assert!(!crate::daemon_roster::roster_path(&home).exists());
        let spec = crate::background_launch::read_launch_spec(&home, &short).unwrap();
        assert_eq!(spec.launch, BackgroundLaunchKind::Fresh);
        assert_eq!(spec.initial_prompt.as_deref(), argv.prompt.as_deref());
        assert_eq!(
            spec.transcript_path,
            session::jsonl::path::session_path(&home, &spec.cwd, &spec.session_id)
                .display()
                .to_string()
        );

        // (b) The captured spawn argv carries the literal `daemon` token in
        //     argv[1..4] (classify_cmdline's recognition window).
        assert_eq!(spawner.spawns.len(), 1);
        let spawned = &spawner.spawns[0];
        assert!(
            spawned.iter().skip(1).take(3).any(|t| t == "daemon"),
            "daemon token in argv[1..4]: {spawned:?}"
        );
    }

    #[test]
    fn dispatch_freezes_foreground_resolved_permission_mode() {
        let home = tmpdir();
        let mut spawner = CaptureSpawner::default();
        let lockp = FakeLockProbe {
            live: HashMap::new(),
        };
        let mut argv = bg_argv("permission snapshot");
        argv.permission_mode = Some("bypassPermissions".to_string());
        argv.dangerously_skip_permissions = true;

        assert_eq!(
            dispatch_background_inner(
                &argv,
                &home,
                &home,
                true,
                permission::PermissionMode::Plan,
                &lockp,
                &mut spawner,
            ),
            exit_codes::SUCCESS
        );
        let short = agents_registry::read_jobs(&agents_registry::jobs_dir(&home))[0]
            .0
            .clone();
        let spec = crate::background_launch::read_launch_spec(&home, &short).unwrap();
        assert_eq!(spec.options.permission_mode.as_deref(), Some("plan"));
        assert!(!spec.options.dangerously_skip_permissions);
        let hidden_argv = spec.tui_argv();
        assert_eq!(hidden_argv.permission_mode.as_deref(), Some("plan"));
        assert!(!hidden_argv.dangerously_skip_permissions);
    }

    #[test]
    fn startup_preflight_receives_the_foreground_resolved_permission_mode() {
        let mut argv = bg_argv("permission preflight");
        argv.permission_mode = Some("bypassPermissions".to_string());
        argv.dangerously_skip_permissions = true;

        let resolved =
            argv_with_resolved_permission_mode(&argv, permission::PermissionMode::Default);
        assert_eq!(resolved.permission_mode.as_deref(), Some("default"));
        assert!(!resolved.dangerously_skip_permissions);
        assert_eq!(argv.permission_mode.as_deref(), Some("bypassPermissions"));
        assert!(argv.dangerously_skip_permissions);
    }

    /// (2.1.212 G06) resume-AS-background reuses `dispatch_forked_session` for an
    /// EXISTING session id (no snapshot write): it must record a resume/fork
    /// launch spec pointing at that session's transcript and spawn the daemon — the exact
    /// path `CliBgSessionForker::resume_to_background` drives.
    #[test]
    fn forked_dispatch_records_resume_job_for_existing_session() {
        let home = tmpdir();
        let mut spawner = CaptureSpawner::default();
        let lockp = FakeLockProbe {
            live: HashMap::new(),
        };
        let session_id = "11111111-2222-3333-4444-555555555555";
        let transcript = session::jsonl::path::session_path(&home, "/tmp/proj", session_id);
        std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        std::fs::write(
            &transcript,
            format!("{{\"type\":\"user\",\"sessionId\":\"{session_id}\",\"cwd\":\"/tmp/proj\"}}\n"),
        )
        .unwrap();
        // Empty prompt = resume-as-bg (no seed turn), unlike a fork with a `[prompt]`.
        let short = dispatch_forked_session_inner(
            &home,
            &home,
            "/tmp/proj",
            session_id,
            "",
            &lockp,
            &mut spawner,
        )
        .expect("forked dispatch should succeed");

        // A job row exists for the resumed session.
        let jobs = agents_registry::read_jobs(&agents_registry::jobs_dir(&home));
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].1.session_id.as_deref(), Some(session_id));
        assert_eq!(jobs[0].1.backend.as_deref(), Some("daemon"));
        // No seed prompt ⇒ no intent/initial_prompt for a bare resume.
        assert_eq!(jobs[0].1.initial_prompt.as_deref(), None);

        assert!(!crate::daemon_roster::roster_path(&home).exists());
        let spec = crate::background_launch::read_launch_spec(&home, &short).unwrap();
        assert_eq!(spec.launch, BackgroundLaunchKind::Fork);
        assert_eq!(spec.session_id, session_id);
        let expected = session::jsonl::path::session_path(&home, "/tmp/proj", session_id);
        assert_eq!(
            std::fs::canonicalize(&spec.transcript_path).unwrap(),
            std::fs::canonicalize(expected).unwrap()
        );
        // The daemon is spawned.
        assert_eq!(spawner.spawns.len(), 1);
    }

    #[test]
    fn fork_dispatch_keeps_source_transcript_separate_from_target_session() {
        let home = tmpdir();
        let mut spawner = CaptureSpawner::default();
        let lockp = FakeLockProbe {
            live: HashMap::new(),
        };
        let source_id = "10101010-2222-3333-4444-555555555555";
        let target_id = "20202020-2222-3333-4444-555555555555";
        let source = session::jsonl::path::session_path(&home, "/tmp/source", source_id);
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(
            &source,
            format!(
                "{{\"type\":\"user\",\"sessionId\":\"{source_id}\",\"cwd\":\"/tmp/source\"}}\n"
            ),
        )
        .unwrap();
        let target = prepare_resume_transcript(&home, "/tmp/target", source_id, target_id, true)
            .expect("fork should copy from source before resolving the target");
        assert_ne!(std::fs::canonicalize(&source).unwrap(), target);
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            std::fs::read_to_string(&source).unwrap()
        );
        let context = ForkLaunchContext {
            transcript_path: Some(target.display().to_string()),
            options: Some(BackgroundLaunchOptions {
                model: Some("captured-model".to_string()),
                system_prompt: Some("captured system".to_string()),
                ..BackgroundLaunchOptions::default()
            }),
            handoff: Some(traits::BackgroundingSnapshot::Idle {
                queued_commands: vec!["/compact".to_string()],
                draft: "保留这段草稿".to_string(),
                boundary_id: uuid::Uuid::nil(),
            }),
            ..ForkLaunchContext::default()
        };

        let short = dispatch_resumed_session_inner(
            &home,
            &home,
            "/tmp/target",
            target_id,
            "",
            true,
            &context,
            &lockp,
            &mut spawner,
        )
        .expect("fork should locate the source transcript independently");

        let spec = crate::background_launch::read_launch_spec(&home, &short).unwrap();
        assert_eq!(spec.session_id, target_id);
        assert_eq!(
            std::fs::canonicalize(&spec.transcript_path).unwrap(),
            target
        );
        assert_eq!(spec.options.model.as_deref(), Some("captured-model"));
        assert_eq!(
            spec.options.system_prompt.as_deref(),
            Some("captured system")
        );
        let handoff = spec
            .handoff
            .as_ref()
            .expect("mid-turn handoff must survive daemon launch persistence");
        assert_eq!(handoff.queued_commands(), &["/compact"]);
        assert_eq!(handoff.draft(), "保留这段草稿");
    }

    #[test]
    fn resume_locator_preserves_exact_cross_cwd_transcript_and_origin() {
        let home = tmpdir();
        let session_id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let original_cwd = "/tmp/original-worktree";
        let transcript = session::jsonl::path::session_path(&home, original_cwd, session_id);
        std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        std::fs::write(
            &transcript,
            format!(
                "{{\"type\":\"user\",\"sessionId\":\"{session_id}\",\"cwd\":\"{original_cwd}\"}}\n"
            ),
        )
        .unwrap();

        let located = locate_session_transcript(&home, "/tmp/different", session_id).unwrap();
        assert_eq!(located, std::fs::canonicalize(&transcript).unwrap());
        assert_eq!(transcript_cwd(&located).as_deref(), Some(original_cwd));
    }

    #[cfg(unix)]
    #[test]
    fn resume_locator_rejects_symlink_substitution() {
        use std::os::unix::fs::symlink;

        let home = tmpdir();
        let session_id = "aaaaaaaa-bbbb-cccc-dddd-ffffffffffff";
        let transcript = session::jsonl::path::session_path(&home, "/tmp/project", session_id);
        std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        let outside = home.join("outside.jsonl");
        std::fs::write(&outside, b"{}\n").unwrap();
        symlink(&outside, &transcript).unwrap();
        assert!(locate_session_transcript(&home, "/tmp/project", session_id).is_err());
    }

    #[test]
    fn skips_spawn_when_a_live_daemon_holds_the_lock() {
        let home = tmpdir();
        // A live supervisor already owns the lock.
        let mut held = daemon_lock::DaemonLock::new(5555, "0.0.0");
        held.proc_start = Some("S".to_string());
        daemon_lock::acquire(&home, &held).unwrap();

        let mut live = HashMap::new();
        live.insert(5555, true);
        let lockp = FakeLockProbe { live };
        let mut spawner = CaptureSpawner::default();

        let code = dispatch_background_inner(
            &bg_argv("hello"),
            &home,
            &home,
            true,
            permission::PermissionMode::Default,
            &lockp,
            &mut spawner,
        );
        assert_eq!(code, exit_codes::SUCCESS);
        // Job + launch spec are still written, but no daemon is spawned (live holder).
        assert_eq!(
            agents_registry::read_jobs(&agents_registry::jobs_dir(&home)).len(),
            1
        );
        assert!(
            spawner.spawns.is_empty(),
            "no spawn when a live daemon holds the lock"
        );
    }

    #[test]
    fn dispatch_records_env_and_wraps_daemon_spawn() {
        let _guard = env_lock().lock().unwrap();
        std::env::set_var("LINGXI_CODE_PROCESS_WRAPPER", "/tmp/wrap --trace");
        std::env::set_var("ANTHROPIC_API_KEY", "launch-only-secret");

        let home = tmpdir();
        let mut spawner = CaptureSpawner::default();
        let lockp = FakeLockProbe {
            live: HashMap::new(),
        };
        let code = dispatch_background_inner(
            &bg_argv("hello"),
            &home,
            &home,
            true,
            permission::PermissionMode::Default,
            &lockp,
            &mut spawner,
        );

        std::env::remove_var("LINGXI_CODE_PROCESS_WRAPPER");
        std::env::remove_var("ANTHROPIC_API_KEY");

        assert_eq!(code, exit_codes::SUCCESS);
        let jobs = agents_registry::read_jobs(&agents_registry::jobs_dir(&home));
        let short = jobs.first().expect("background job").0.clone();
        assert!(!crate::daemon_roster::roster_path(&home).exists());
        let spec = crate::background_launch::read_launch_spec(&home, &short).unwrap();
        assert_eq!(
            spec.env
                .get("LINGXI_CODE_PROCESS_WRAPPER")
                .map(String::as_str),
            Some("/tmp/wrap --trace")
        );
        assert_eq!(
            spec.env.get("ANTHROPIC_API_KEY").map(String::as_str),
            Some("launch-only-secret")
        );

        let spawned = spawner.spawns.first().expect("daemon spawn");
        assert_eq!(spawned.first().map(String::as_str), Some("/tmp/wrap"));
        assert!(
            spawned.iter().skip(1).take(4).any(|t| t == "daemon"),
            "daemon token preserved after wrapper: {spawned:?}"
        );
    }

    #[test]
    fn empty_prompt_yields_no_intent() {
        assert_eq!(intent_from_prompt(Some("   ")), None);
        assert_eq!(intent_from_prompt(None), None);
        assert_eq!(
            intent_from_prompt(Some("first\nsecond")).as_deref(),
            Some("first")
        );
    }

    #[test]
    fn worktree_launch_is_named_once_and_bound_into_the_launch_path() {
        let options = BackgroundLaunchOptions {
            worktree: Some(String::new()),
            tmux: Some("classic".to_string()),
            ..BackgroundLaunchOptions::default()
        };
        let (options, path) = normalize_launch_options(options, "/repo").unwrap();
        let slug = options.worktree.as_deref().unwrap();
        let expected = format!("/repo/.lingxi/worktrees/{slug}");
        assert!(slug.starts_with("session-"));
        assert_eq!(options.tmux.as_deref(), Some("classic"));
        assert_eq!(path.as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn worktree_launch_rejects_path_traversal_and_tmux_without_worktree() {
        let traversal = BackgroundLaunchOptions {
            worktree: Some("../escape".to_string()),
            ..BackgroundLaunchOptions::default()
        };
        assert!(normalize_launch_options(traversal, "/repo").is_err());

        let tmux_only = BackgroundLaunchOptions {
            tmux: Some(String::new()),
            ..BackgroundLaunchOptions::default()
        };
        assert_eq!(
            normalize_launch_options(tmux_only, "/repo").unwrap_err(),
            "--tmux requires --worktree"
        );
    }

    #[test]
    fn resumed_worktree_sessions_preserve_root_metadata_but_strip_boot_flags() {
        let home = tmpdir();
        let mut spawner = CaptureSpawner::default();
        let lockp = FakeLockProbe {
            live: HashMap::new(),
        };
        let worktree_root = format!("/tmp/repo/{}/worktrees/feature", branding::DOT_DIR);
        let worktree_cwd = format!("{worktree_root}/src");
        let context = ForkLaunchContext {
            options: Some(BackgroundLaunchOptions {
                worktree: Some("feature".to_string()),
                tmux: Some("classic".to_string()),
                ..BackgroundLaunchOptions::default()
            }),
            ..ForkLaunchContext::default()
        };

        for (fork, dispatch_cwd, session_id) in [
            (
                false,
                "/tmp/different",
                "11111111-2222-3333-4444-555555555555",
            ),
            (
                true,
                worktree_cwd.as_str(),
                "66666666-7777-8888-9999-aaaaaaaaaaaa",
            ),
        ] {
            let transcript = session::jsonl::path::session_path(&home, &worktree_cwd, session_id);
            std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
            std::fs::write(
                &transcript,
                format!(
                    "{{\"type\":\"user\",\"sessionId\":\"{session_id}\",\"cwd\":\"{worktree_cwd}\"}}\n"
                ),
            )
            .unwrap();

            let short = dispatch_resumed_session_inner(
                &home,
                &home,
                dispatch_cwd,
                session_id,
                "",
                fork,
                &context,
                &lockp,
                &mut spawner,
            )
            .expect("resume/fork dispatch should succeed");

            let spec = crate::background_launch::read_launch_spec(&home, &short).unwrap();
            assert_eq!(spec.cwd, worktree_cwd);
            assert_eq!(spec.worktree_path.as_deref(), Some(worktree_root.as_str()));
            assert_eq!(spec.options.worktree, None);
            assert_eq!(spec.options.tmux, None);
            let argv = spec.tui_argv();
            assert_eq!(argv.worktree, None);
            assert_eq!(argv.tmux, None);
        }
    }

    #[test]
    fn launch_environment_keeps_runtime_context_but_rejects_worker_identity() {
        assert!(launch_env_key_allowed("CLAUDE_CODE_USE_BEDROCK"));
        assert!(launch_env_key_allowed("LINGXI_AUTOCOMPACT_PCT_OVERRIDE"));
        assert!(launch_env_key_allowed("AWS_SECRET_ACCESS_KEY"));
        assert!(launch_env_key_allowed("XDG_CONFIG_HOME"));
        assert!(launch_env_key_allowed("TERM_PROGRAM"));
        assert!(launch_env_key_allowed("TERM_PROGRAM_VERSION"));
        assert!(launch_env_key_allowed("KITTY_WINDOW_ID"));
        assert!(launch_env_key_allowed("WT_SESSION"));
        assert!(launch_env_key_allowed("TERMINAL_EMULATOR"));
        assert!(launch_env_key_allowed("LC_TERMINAL"));
        assert!(!launch_env_key_allowed("LINGXI_BG_ATTACH_AUTH"));
        assert!(!launch_env_key_allowed("LINGXI_BG_SESSION"));
        assert!(!launch_env_key_allowed("UNRELATED_AMBIENT_VALUE"));
    }
}
