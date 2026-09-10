//! `lingxi-cli rm <id>` — Delete a background session and its worktree
//! (cc 2.1.207 P2-11 remainder, dossier fix #3: the permanent-deletion
//! lifecycle).
//!
//! Byte-faithful surface for the real 2.1.207 `claude rm <id>` command
//! (verified via `strings`/byte-dumps of the binary):
//!
//! * `-h`/`--help` → the full usage + description block (stdout, exit 0):
//!   `Usage: claude rm <id>\n\n  Delete a background session and its worktree.
//!   Unlike `stop`, works on already-exited sessions.\n`
//! * missing `<id>` → stderr bare `Usage: claude rm <id>`, exit 1.
//! * `<id>` prefix-matches the `jobs/<short>` dirs
//!   ([`crate::agents_registry`]): no match → stderr `No job matching
//!   '<id>'. Run 'claude agents' to list running sessions.` exit 1; an
//!   ambiguous prefix → stderr `Ambiguous prefix '<id>', matches: <a>, <b>`
//!   exit 1 (matches sorted, joined with `, `).
//! * a still-running worker that cannot be confirmed stopped → stderr
//!   `couldn't confirm <id> was stopped — the background service may be
//!   restarting. Try again in a moment.` exit 1.
//! * otherwise the `jobs/<short>` state dir is removed (and, when the job
//!   carried a managed worktree that could not be removed, retained with the
//!   binary's `keptReason` text appended); stdout `removed <id>` (+ the
//!   worktree-kept suffix when applicable), exit 0.
//!
//! Telemetry: `tengu_bg_agent_action{action:"delete",source:"cli"}` and the
//! CLI analytics event `cli_bg_rm` (both parity-name only — routed through the
//! `tracing` event sink like `daemon.rs`'s `tengu_bg_*` emissions).
//!
//! The `claude` product name is kept in the user-typed recipe strings (help,
//! `Run 'claude agents'`) exactly as the binary/fixtures pin it — the same
//! convention `argv.rs` uses for its `claude --bg`/`claude agents` recipes and
//! `gateway.rs` uses for its `Usage: claude gateway` help.

use clap::Args;
use std::path::{Path, PathBuf};

/// The locked `claude rm --help` text — byte-identical to the 2.1.207 binary
/// (`Usage: claude rm <id>\n\n  Delete a background session and its worktree.
/// Unlike `stop`, works on already-exited sessions.\n`).
pub const RM_HELP: &str = "Usage: claude rm <id>\n\n  Delete a background session and its worktree. Unlike `stop`, works on already-exited sessions.\n";

/// The bare usage line (missing `<id>` / arg error), stderr, exit 1.
pub const RM_USAGE: &str = "Usage: claude rm <id>";

/// `rm` args — a single positional `<id>` (declared optional so the missing-arg
/// path can emit the bare `Usage:` line byte-exactly instead of clap's own
/// required-arg rendering) plus a manual `-h/--help` flag (help auto-flag
/// disabled, same idiom as `gateway.rs`).
#[derive(Debug, Clone, Args)]
#[command(disable_help_flag = true)]
pub struct Cli {
    /// Display help for command (manual: prints the locked help text).
    #[arg(short = 'h', long = "help")]
    pub help: bool,

    /// The background session id (prefix) to delete.
    #[arg(value_name = "id")]
    pub id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManagedWorktreeBinding {
    short: String,
    session_id: String,
    canonical_path: PathBuf,
    repo_root: PathBuf,
    ownership_token: String,
    identity: worktree_delete::WorktreeIdentity,
}

impl ManagedWorktreeBinding {
    fn new(
        short: &str,
        session_id: &str,
        path: &Path,
        ownership_token: &str,
    ) -> Result<Self, String> {
        let canonical_path = crate::daemon_roster::canonicalize_managed_worktree_path(path)
            .map_err(|_| {
                format!(
                    "Failed to verify managed-worktree deletion for {short}: managed worktree path is invalid."
                )
            })?;
        let repo_root = managed_worktree_repo_root(&canonical_path).ok_or_else(|| {
            format!(
                "Failed to verify managed-worktree deletion for {short}: managed worktree path mismatch."
            )
        })?;
        let identity = worktree_delete::capture_identity(&canonical_path).map_err(|_| {
            format!(
                "Failed to verify managed-worktree deletion for {short}: managed worktree path is invalid."
            )
        })?;
        Ok(Self {
            short: short.to_string(),
            session_id: session_id.to_string(),
            canonical_path,
            repo_root,
            ownership_token: ownership_token.to_string(),
            identity,
        })
    }
}

fn managed_worktree_repo_root(path: &Path) -> Option<PathBuf> {
    let worktrees = path.parent()?;
    if worktrees.file_name()? != "worktrees" {
        return None;
    }
    let dot_dir = worktrees.parent()?;
    if dot_dir.file_name()? != branding::DOT_DIR {
        return None;
    }
    Some(dot_dir.parent()?.to_path_buf())
}

fn resolve_managed_worktree_binding(
    runtime_dir: &Path,
    short: &str,
) -> Result<Option<ManagedWorktreeBinding>, String> {
    let roster = crate::daemon_roster::read_roster(runtime_dir, 0, false).into_roster();
    if let Some(record) = roster.workers.get(short) {
        if let Some(worktree) = record.dispatch.worktree.as_ref() {
            return ManagedWorktreeBinding::new(
                short,
                &record.session_id,
                Path::new(&worktree.path),
                &worktree.ownership_token,
            )
            .map(Some);
        }
    }

    match crate::background_launch::read_launch_spec(runtime_dir, short) {
        Ok(spec) => match (
            spec.worktree_path.as_deref(),
            spec.worktree_ownership_token.as_deref(),
        ) {
            (Some(path), Some(token)) => ManagedWorktreeBinding::new(
                short,
                &spec.session_id,
                Path::new(path),
                token,
            )
            .map(Some),
            (Some(_), None) => Err(format!(
                "Failed to verify managed-worktree deletion for {short}: unsafe legacy ownership metadata."
            )),
            _ => Ok(None),
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!(
            "Failed to verify managed-worktree deletion for {short}: could not read trusted ownership metadata ({err})."
        )),
    }
}

pub(crate) fn verify_managed_worktree_token(
    runtime_dir: &Path,
    short: &str,
    job: &crate::agents_registry::JobState,
) -> Result<(), String> {
    let job_path =
        managed_worktree_path(job).or_else(|| launch_worktree_path(runtime_dir, short, job));
    let maybe_record = resolve_managed_worktree_binding(runtime_dir, short)?;
    let (job_path, recorded) = match (job_path, maybe_record) {
        (Some(job_path), Some(recorded)) => (job_path, recorded),
        (Some(_), None) => {
            return Err(format!(
                "Failed to verify managed-worktree deletion for {short}: unsafe legacy ownership metadata."
            ));
        }
        (None, Some(_)) => {
            return Err(format!(
                "Failed to verify managed-worktree deletion for {short}: worktree path missing from job state."
            ));
        }
        (None, None) => return Ok(()),
    };

    if !job_path.exists() {
        return Ok(());
    }
    let job_path = crate::daemon_roster::canonicalize_managed_worktree_path(&job_path).map_err(|_| {
        format!(
            "Failed to verify managed-worktree deletion for {short}: managed worktree path is invalid."
        )
    })?;
    if job_path != recorded.canonical_path {
        return Err(format!(
            "Failed to verify managed-worktree deletion for {short}: worktree path mismatch."
        ));
    }

    let marker = crate::daemon_roster::read_worktree_ownership_marker(&recorded.canonical_path)
        .map_err(|e| {
            format!(
                "Failed to verify managed-worktree deletion for {short}: missing or unreadable ownership marker ({e})."
            )
        })?;
    if marker.schema_version != crate::daemon_roster::WORKTREE_OWNERSHIP_MARKER_SCHEMA_VERSION {
        return Err(format!(
            "Failed to verify managed-worktree deletion for {short}: unsupported marker schema."
        ));
    }
    if marker.short != short
        || marker.session_id != recorded.session_id
        || marker.ownership_token != recorded.ownership_token
    {
        return Err(format!(
            "Failed to verify managed-worktree deletion for {short}: worktree ownership metadata mismatch."
        ));
    }
    if marker.canonical_worktree_path != recorded.canonical_path.display().to_string() {
        return Err(format!(
            "Failed to verify managed-worktree deletion for {short}: worktree marker path mismatch."
        ));
    }

    Ok(())
}

/// Which `jobs/<short>` the `<id>` prefix resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrefixMatch {
    /// Exactly one job matched — the resolved short id.
    Unique(String),
    /// No job matched the prefix.
    None,
    /// More than one job matched (sorted short ids).
    Ambiguous(Vec<String>),
}

/// Resolve `<id>` against the known `jobs/<short>` ids (the binary prefix-match).
/// An exact id wins outright; otherwise every short with `id` as a prefix is a
/// candidate. `shorts` is assumed already sorted ([`crate::agents_registry::read_jobs`]
/// sorts ascending); the returned `Ambiguous` list preserves that order.
#[must_use]
pub fn match_prefix(shorts: &[String], id: &str) -> PrefixMatch {
    if shorts.iter().any(|s| s == id) {
        return PrefixMatch::Unique(id.to_string());
    }
    let matches: Vec<String> = shorts
        .iter()
        .filter(|s| s.starts_with(id))
        .cloned()
        .collect();
    match matches.len() {
        0 => PrefixMatch::None,
        1 => PrefixMatch::Unique(matches.into_iter().next().unwrap()),
        _ => PrefixMatch::Ambiguous(matches),
    }
}

/// The reason a job's managed worktree was kept rather than removed — the
/// binary `keptReason` map. Byte-exact reason strings; only `RemoveFailed` is
/// reachable from lingxi today (worktree dirty/branch/lock detection needs a
/// git+lock probe lingxi does not run here), but all four are pinned so the
/// surface matches the oracle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeptReason {
    /// `dirty` → the worktree has uncommitted changes.
    Dirty,
    /// `branch_mismatch` → the worktree is on a different branch.
    BranchMismatch,
    /// `live_lock` → the worktree is locked (live session / by hand).
    LiveLock,
    /// `remove_failed` → the removal itself errored.
    RemoveFailed,
}

impl KeptReason {
    /// The binary's `keptReason` phrase for this reason.
    #[must_use]
    pub fn text(self) -> &'static str {
        match self {
            KeptReason::Dirty => "has uncommitted changes",
            KeptReason::BranchMismatch => "is on a different branch",
            KeptReason::LiveLock => {
                "is locked \u{2014} in use by another live session, or locked by hand"
            }
            KeptReason::RemoveFailed => "could not be removed",
        }
    }
}

/// `No job matching '<id>'. Run 'claude agents' to list running sessions.`
#[must_use]
pub fn no_job_message(id: &str) -> String {
    format!("No job matching '{id}'. Run 'claude agents' to list running sessions.")
}

/// `Ambiguous prefix '<id>', matches: <a>, <b>`
#[must_use]
pub fn ambiguous_message(id: &str, matches: &[String]) -> String {
    format!("Ambiguous prefix '{id}', matches: {}", matches.join(", "))
}

/// `couldn't confirm <id> was stopped — the background service may be
/// restarting. Try again in a moment.`
#[must_use]
pub fn couldnt_confirm_message(short: &str) -> String {
    format!(
        "couldn't confirm {short} was stopped \u{2014} the background service may be restarting. Try again in a moment."
    )
}

/// `removed <id>` with the optional worktree-kept suffix
/// (`\n  worktree <reason> — kept at <path>`).
#[must_use]
pub fn removed_message(short: &str, kept: Option<(KeptReason, &str)>) -> String {
    match kept {
        None => format!("removed {short}"),
        Some((reason, path)) => format!(
            "removed {short}\n  worktree {} \u{2014} kept at {path}",
            reason.text()
        ),
    }
}

/// The managed-worktree path a job carries, if any: its `cwd` when that path is
/// under a `<DOT_DIR>/worktrees/` segment (the layout `job_origin_cwd` strips).
/// A job whose `cwd` is a plain project dir (the common lingxi `--bg` case,
/// where `background_dispatch` leaves `worktree` unset) has no managed worktree.
#[must_use]
pub fn managed_worktree_path(job: &crate::agents_registry::JobState) -> Option<PathBuf> {
    let cwd = job.cwd.as_deref()?;
    for sep in ['/', '\\'] {
        let marker = format!("{sep}{}{sep}worktrees{sep}", branding::DOT_DIR);
        if cwd.contains(&marker) {
            return Some(PathBuf::from(cwd));
        }
    }
    None
}

fn launch_worktree_path(
    home: &Path,
    short: &str,
    job: &crate::agents_registry::JobState,
) -> Option<PathBuf> {
    let spec = crate::background_launch::read_launch_spec(home, short).ok()?;
    let candidate = PathBuf::from(spec.worktree_path?);
    let origin = PathBuf::from(job.origin_cwd.as_deref().unwrap_or(&spec.origin_cwd));
    let managed_root = origin.join(branding::DOT_DIR).join("worktrees");
    if candidate.parent() == Some(managed_root.as_path())
        && candidate
            .file_name()
            .is_some_and(|name| !name.is_empty() && name != "." && name != "..")
    {
        Some(candidate)
    } else {
        None
    }
}

/// Remove a resolved `jobs/<short>` state dir and, best-effort, its managed
/// worktree; emit the `delete` telemetry; return the worktree-kept reason (if
/// the worktree survived) so the caller can render the `removed <id>` suffix.
///
/// `source` is the `tengu_bg_agent_action` provenance — `"cli"` from the `rm`
/// command, `"fleet"` from the agents view's Ctrl-X delete. Shared by both so
/// the deletion behaves identically regardless of entry point.
pub fn perform_delete(
    home: &Path,
    short: &str,
    job: &crate::agents_registry::JobState,
    source: &str,
) -> Result<Option<(KeptReason, String)>, String> {
    use crate::agents_registry as reg;

    let verified_before = resolve_verified_worktree_binding(home, short, job)?;
    let deletion = match crate::commands::daemon::claim_and_quiesce_background_job_for_delete(
        home, short, job,
    ) {
        Ok(claim) => claim,
        Err(error) => {
            tracing::warn!(short, %error, "background deletion could not quiesce exact worker");
            if error.contains("state changed") {
                return Err(error);
            }
            tracing::info!(
                event = "cli_bg_rm",
                short = short,
                outcome = "kill_unconfirmed"
            );
            return Err(couldnt_confirm_message(short));
        }
    };
    if live_bg_identity_exists(home, short) {
        tracing::info!(
            event = "cli_bg_rm",
            short = short,
            outcome = "kill_unconfirmed"
        );
        return Err(couldnt_confirm_message(short));
    }

    // Keep the exact delete token stable through worktree verification and the
    // final directory unlink. The lock file lives in `jobs/.locks`, outside the
    // removed job directory, so this is safe on Windows as well as POSIX.
    let _job_lock = reg::lock_job_state(home, short).map_err(|error| error.to_string())?;
    let current = reg::read_job(home, short)
        .ok_or_else(|| "background job disappeared before deletion".to_string())?;
    if current.phase.as_deref() != Some(crate::commands::respawn::PHASE_DELETING)
        || current.claim_token.as_deref() != Some(deletion.token.as_str())
        || current.claim_owner.as_deref() != Some(crate::commands::respawn::CLAIM_OWNER_DELETE)
        || current.worker_pid.is_some()
        || current.worker_proc_start.is_some()
    {
        return Err("session state changed while deletion was finalizing".to_string());
    }
    debug_assert_eq!(current.claim_token, deletion.job.claim_token);
    let verified_after = resolve_verified_worktree_binding(home, short, &current)?;
    if verified_before != verified_after {
        return Err(format!(
            "Failed to verify managed-worktree deletion for {short}: ownership metadata changed before deletion."
        ));
    }

    // Best-effort worktree removal FIRST (a kept worktree is reported in the
    // output; a removed one is silent). Only a path clearly under the managed
    // worktrees dir is ever touched — never the user's project checkout.
    let kept = verified_after.as_ref().and_then(remove_managed_worktree);

    // Remove the job state dir (idempotent — a missing dir is a no-op).
    let dir = reg::jobs_dir(home).join(short);
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("Failed to remove background job {short}: {error}")),
    }

    // Parity-name telemetry (routed through the tracing event sink, like
    // daemon.rs's tengu_bg_* emissions).
    tracing::info!(
        event = "tengu_bg_agent_action",
        action = "delete",
        source,
        short
    );

    Ok(kept)
}

fn resolve_verified_worktree_binding(
    runtime_dir: &Path,
    short: &str,
    job: &crate::agents_registry::JobState,
) -> Result<Option<ManagedWorktreeBinding>, String> {
    verify_managed_worktree_token(runtime_dir, short, job)?;
    resolve_managed_worktree_binding(runtime_dir, short)
}

fn live_bg_identity_exists(runtime_dir: &Path, short: &str) -> bool {
    use crate::daemon_roster::ProcProbe;

    let probe = crate::daemon_roster::SystemProbe;
    let roster = crate::daemon_roster::read_roster(runtime_dir, 0, false).into_roster();
    if roster.workers.get(short).is_some_and(|record| {
        matches!(
            crate::daemon_roster::adopt(&probe, record),
            crate::daemon_roster::AdoptDecision::Adopt
        )
    }) {
        return true;
    }

    let Ok(runtime) = crate::background_launch::read_pty_runtime(runtime_dir, short) else {
        return false;
    };
    let Ok(child_pid) = i32::try_from(runtime.child_pid) else {
        return false;
    };
    runtime
        .child_proc_start
        .as_deref()
        .is_some_and(|expected| probe.start_time(child_pid).as_deref() == Some(expected))
        && probe.is_alive(child_pid)
}

fn remove_managed_worktree(binding: &ManagedWorktreeBinding) -> Option<(KeptReason, String)> {
    let path = &binding.canonical_path;
    if !path.exists() {
        return None;
    }
    let path_display = path.display().to_string();
    let anchored = match worktree_delete::open(binding) {
        Ok(anchored) => anchored,
        Err(_) => return Some((KeptReason::RemoveFailed, path_display)),
    };
    match worktree_lock_status(&anchored) {
        WorktreeLockStatus::Locked => return Some((KeptReason::LiveLock, path_display)),
        WorktreeLockStatus::CannotVerify => {
            return Some((KeptReason::RemoveFailed, path_display));
        }
        WorktreeLockStatus::Unlocked => {}
    }
    match worktree_status(&anchored) {
        WorktreeStatus::Dirty => return Some((KeptReason::Dirty, path_display)),
        WorktreeStatus::BranchMismatch => return Some((KeptReason::BranchMismatch, path_display)),
        WorktreeStatus::CannotVerify => return Some((KeptReason::RemoveFailed, path_display)),
        WorktreeStatus::Clean => {}
    }
    match worktree_delete::remove(binding, &anchored) {
        Ok(()) => {
            let _ = std::process::Command::new("git")
                .arg("-C")
                .arg(&binding.repo_root)
                .arg("worktree")
                .arg("prune")
                .arg("--expire")
                .arg("now")
                .status();
            None
        }
        Err(_) => Some((KeptReason::RemoveFailed, path_display)),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorktreeStatus {
    Clean,
    Dirty,
    BranchMismatch,
    CannotVerify,
}

fn worktree_status(worktree: &worktree_delete::AnchoredWorktree) -> WorktreeStatus {
    let marker_exclude = format!(
        ":(exclude){}",
        crate::daemon_roster::WORKTREE_OWNERSHIP_MARKER
    );
    let status =
        worktree.git_output(&["status", "--porcelain", "--", ".", marker_exclude.as_str()]);
    let Ok(status) = status else {
        return WorktreeStatus::CannotVerify;
    };
    if !status.status.success() {
        return WorktreeStatus::CannotVerify;
    }
    if !String::from_utf8_lossy(&status.stdout).trim().is_empty() {
        return WorktreeStatus::Dirty;
    }

    let branch = worktree.git_output(&["symbolic-ref", "--quiet", "--short", "HEAD"]);
    let Ok(branch) = branch else {
        return WorktreeStatus::CannotVerify;
    };
    if !branch.status.success() {
        return WorktreeStatus::CannotVerify;
    }
    let branch = String::from_utf8_lossy(&branch.stdout).trim().to_string();
    let Some(expected) = worktree
        .path()
        .file_name()
        .and_then(|value| value.to_str())
        .map(|value| format!("worktree-{value}"))
    else {
        return WorktreeStatus::CannotVerify;
    };
    if branch != expected {
        WorktreeStatus::BranchMismatch
    } else {
        WorktreeStatus::Clean
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorktreeLockStatus {
    Locked,
    Unlocked,
    CannotVerify,
}

fn worktree_lock_status(worktree: &worktree_delete::AnchoredWorktree) -> WorktreeLockStatus {
    let output = worktree.git_output(&[
        "rev-parse",
        "--path-format=absolute",
        "--git-path",
        "locked",
    ]);
    let Ok(output) = output else {
        return WorktreeLockStatus::CannotVerify;
    };
    if !output.status.success() {
        return WorktreeLockStatus::CannotVerify;
    }
    let locked = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if locked.is_empty() {
        return WorktreeLockStatus::CannotVerify;
    }
    if Path::new(&locked).exists() {
        WorktreeLockStatus::Locked
    } else {
        WorktreeLockStatus::Unlocked
    }
}

#[cfg(unix)]
mod worktree_delete {
    use super::ManagedWorktreeBinding;
    use rustix::fs::{self, AtFlags, Dir, FileType, Mode, OFlags};
    use std::os::fd::OwnedFd;
    use std::os::unix::fs::MetadataExt;
    use std::path::{Path, PathBuf};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) struct WorktreeIdentity {
        dev: u64,
        ino: u64,
    }

    impl WorktreeIdentity {
        fn from_metadata(metadata: &std::fs::Metadata) -> Self {
            Self {
                dev: metadata.dev(),
                ino: metadata.ino(),
            }
        }

        fn from_stat(stat: &rustix::fs::Stat) -> Self {
            Self {
                dev: u64::try_from(stat.st_dev).unwrap_or(u64::MAX),
                ino: stat.st_ino,
            }
        }
    }

    pub(super) struct AnchoredWorktree {
        path: PathBuf,
        fd: OwnedFd,
    }

    impl AnchoredWorktree {
        pub(super) fn git_output(&self, args: &[&str]) -> std::io::Result<std::process::Output> {
            let mut command = std::process::Command::new("git");
            command.args(args);
            let directory = std::fs::File::from(self.fd.try_clone()?);
            platform_pty::command_current_dir_from_open_directory(&mut command, &directory)?;
            command.output()
        }

        pub(super) fn path(&self) -> &Path {
            &self.path
        }
    }

    pub(super) fn capture_identity(path: &Path) -> std::io::Result<WorktreeIdentity> {
        let metadata = std::fs::metadata(path)?;
        Ok(WorktreeIdentity::from_metadata(&metadata))
    }

    pub(super) fn open(binding: &ManagedWorktreeBinding) -> std::io::Result<AnchoredWorktree> {
        let fd = fs::open(
            &binding.canonical_path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?;
        let opened = WorktreeIdentity::from_stat(&fs::fstat(&fd).map_err(std::io::Error::from)?);
        if opened != binding.identity {
            return Err(std::io::Error::other(
                "managed worktree identity changed before deletion",
            ));
        }
        Ok(AnchoredWorktree {
            path: binding.canonical_path.clone(),
            fd,
        })
    }

    pub(super) fn remove(
        binding: &ManagedWorktreeBinding,
        anchored: &AnchoredWorktree,
    ) -> std::io::Result<()> {
        remove_dir_contents(&anchored.fd)?;

        let parent_path = anchored
            .path
            .parent()
            .ok_or_else(|| std::io::Error::other("managed worktree parent missing"))?;
        let leaf_name = anchored
            .path
            .file_name()
            .ok_or_else(|| std::io::Error::other("managed worktree leaf missing"))?;
        let parent = fs::open(
            parent_path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?;
        let current = fs::statat(&parent, leaf_name, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(std::io::Error::from)?;
        if WorktreeIdentity::from_stat(&current) != binding.identity {
            return Err(std::io::Error::other(
                "managed worktree identity changed before final unlink",
            ));
        }
        fs::unlinkat(&parent, leaf_name, AtFlags::REMOVEDIR).map_err(std::io::Error::from)?;
        Ok(())
    }

    fn remove_dir_contents(dir_fd: &OwnedFd) -> std::io::Result<()> {
        let mut dir = Dir::read_from(dir_fd).map_err(std::io::Error::from)?;
        while let Some(entry) = dir.next() {
            let entry = entry.map_err(std::io::Error::from)?;
            let name = entry.file_name();
            if name.to_bytes() == b"." || name.to_bytes() == b".." {
                continue;
            }
            let stat = fs::statat(dir_fd, name, AtFlags::SYMLINK_NOFOLLOW)
                .map_err(std::io::Error::from)?;
            let child_identity = WorktreeIdentity::from_stat(&stat);
            if FileType::from_raw_mode(stat.st_mode) == FileType::Directory {
                let child = fs::openat(
                    dir_fd,
                    name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(std::io::Error::from)?;
                let reopened =
                    WorktreeIdentity::from_stat(&fs::fstat(&child).map_err(std::io::Error::from)?);
                if reopened != child_identity {
                    return Err(std::io::Error::other(
                        "managed worktree child identity changed during deletion",
                    ));
                }
                remove_dir_contents(&child)?;
                let current = fs::statat(dir_fd, name, AtFlags::SYMLINK_NOFOLLOW)
                    .map_err(std::io::Error::from)?;
                if WorktreeIdentity::from_stat(&current) != child_identity {
                    return Err(std::io::Error::other(
                        "managed worktree child identity changed before unlink",
                    ));
                }
                fs::unlinkat(dir_fd, name, AtFlags::REMOVEDIR).map_err(std::io::Error::from)?;
            } else {
                let current = fs::statat(dir_fd, name, AtFlags::SYMLINK_NOFOLLOW)
                    .map_err(std::io::Error::from)?;
                if WorktreeIdentity::from_stat(&current) != child_identity {
                    return Err(std::io::Error::other(
                        "managed worktree child identity changed before unlink",
                    ));
                }
                fs::unlinkat(dir_fd, name, AtFlags::empty()).map_err(std::io::Error::from)?;
            }
        }
        Ok(())
    }
}

#[cfg(windows)]
mod worktree_delete {
    use super::ManagedWorktreeBinding;
    use platform_pty::{
        delete_windows_path_by_handle, enumerate_directory_by_handle, open_child_by_id,
        open_windows_reparse_guarded, windows_file_identity, WindowsFileIdentity,
    };
    use std::path::{Path, PathBuf};

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) struct WorktreeIdentity {
        file_id: WindowsFileIdentity,
    }

    pub(super) struct AnchoredWorktree {
        path: PathBuf,
        handle: std::fs::File,
    }

    impl AnchoredWorktree {
        pub(super) fn git_output(&self, args: &[&str]) -> std::io::Result<std::process::Output> {
            self.verify_path_identity()?;
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(&self.path)
                .args(args)
                .output()?;
            self.verify_path_identity()?;
            Ok(output)
        }

        fn verify_path_identity(&self) -> std::io::Result<()> {
            let current = open_windows_reparse_guarded(&self.path, false)?;
            if windows_file_identity(&current)? != windows_file_identity(&self.handle)? {
                return Err(std::io::Error::other(
                    "managed worktree identity changed while inspecting git state",
                ));
            }
            Ok(())
        }

        pub(super) fn path(&self) -> &Path {
            &self.path
        }
    }

    pub(super) fn capture_identity(path: &Path) -> std::io::Result<WorktreeIdentity> {
        let handle = open_windows_reparse_guarded(path, false)?;
        Ok(WorktreeIdentity {
            file_id: windows_file_identity(&handle)?,
        })
    }

    pub(super) fn open(binding: &ManagedWorktreeBinding) -> std::io::Result<AnchoredWorktree> {
        let handle = open_windows_reparse_guarded(&binding.canonical_path, true)?;
        let opened = WorktreeIdentity {
            file_id: windows_file_identity(&handle)?,
        };
        if opened != binding.identity {
            return Err(std::io::Error::other(
                "managed worktree identity changed before deletion",
            ));
        }
        Ok(AnchoredWorktree {
            path: binding.canonical_path.clone(),
            handle,
        })
    }

    pub(super) fn remove(
        binding: &ManagedWorktreeBinding,
        anchored: &AnchoredWorktree,
    ) -> std::io::Result<()> {
        remove_dir_contents(&anchored.handle)?;
        let current = capture_identity(&binding.canonical_path)?;
        if current != binding.identity {
            return Err(std::io::Error::other(
                "managed worktree identity changed before final unlink",
            ));
        }
        delete_windows_path_by_handle(&anchored.handle)
    }

    fn remove_dir_contents(dir: &std::fs::File) -> std::io::Result<()> {
        for entry in enumerate_directory_by_handle(dir)? {
            let child_handle = open_child_by_id(dir, entry.file_id, true)?;
            let child_identity = WorktreeIdentity {
                file_id: windows_file_identity(&child_handle)?,
            };
            let metadata = child_handle.metadata()?;
            if metadata.is_dir() {
                remove_dir_contents(&child_handle)?;
                delete_windows_path_by_handle(&child_handle)?;
            } else {
                let current = WorktreeIdentity {
                    file_id: windows_file_identity(&child_handle)?,
                };
                if current != child_identity {
                    return Err(std::io::Error::other(
                        "managed worktree child identity changed before unlink",
                    ));
                }
                delete_windows_path_by_handle(&child_handle)?;
            }
        }
        Ok(())
    }
}

#[cfg(not(any(unix, windows)))]
mod worktree_delete {
    use super::ManagedWorktreeBinding;
    use std::path::{Path, PathBuf};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) struct WorktreeIdentity;

    pub(super) struct AnchoredWorktree {
        path: PathBuf,
    }

    impl AnchoredWorktree {
        pub(super) fn git_output(&self, args: &[&str]) -> std::io::Result<std::process::Output> {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&self.path)
                .args(args)
                .output()
        }

        pub(super) fn path(&self) -> &Path {
            &self.path
        }
    }

    pub(super) fn capture_identity(_path: &Path) -> std::io::Result<WorktreeIdentity> {
        Ok(WorktreeIdentity)
    }

    pub(super) fn open(binding: &ManagedWorktreeBinding) -> std::io::Result<AnchoredWorktree> {
        Ok(AnchoredWorktree {
            path: binding.canonical_path.clone(),
        })
    }

    pub(super) fn remove(
        _binding: &ManagedWorktreeBinding,
        anchored: &AnchoredWorktree,
    ) -> std::io::Result<()> {
        std::fs::remove_dir_all(&anchored.path)
    }
}

/// Run the `rm` family (see the module doc for the binary-verified surface).
pub async fn run(cli: &Cli) -> i32 {
    use crate::agents_registry as reg;

    if cli.help {
        // Full help block, stdout, exit 0 (commander help path).
        print!("{RM_HELP}");
        return crate::exit_codes::SUCCESS;
    }
    let Some(id) = cli.id.as_deref().filter(|s| !s.is_empty()) else {
        // Missing `<id>` — bare usage, stderr, exit 1.
        eprintln!("{RM_USAGE}");
        return crate::exit_codes::ARGV_ERROR;
    };

    let home = crate::run::lingxi_home_dir();
    let jobs = reg::read_jobs(&reg::jobs_dir(&home));
    let shorts: Vec<String> = jobs.iter().map(|(s, _)| s.clone()).collect();

    let short = match match_prefix(&shorts, id) {
        PrefixMatch::None => {
            eprintln!("{}", no_job_message(id));
            return crate::exit_codes::RUNTIME_ERROR;
        }
        PrefixMatch::Ambiguous(matches) => {
            eprintln!("{}", ambiguous_message(id, &matches));
            return crate::exit_codes::RUNTIME_ERROR;
        }
        PrefixMatch::Unique(short) => short,
    };

    let job = jobs
        .iter()
        .find(|(s, _)| *s == short)
        .map(|(_, j)| j.clone())
        .unwrap_or_default();

    match perform_delete(&home, &short, &job, "cli") {
        Ok(kept) => {
            tracing::info!(event = "cli_bg_rm", short = short.as_str(), removed = true);
            println!(
                "{}",
                removed_message(&short, kept.as_ref().map(|(r, p)| (*r, p.as_str())))
            );
            crate::exit_codes::SUCCESS
        }
        Err(reason) => {
            eprintln!("{reason}");
            crate::exit_codes::RUNTIME_ERROR
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents_registry::{jobs_dir, write_job_state, JobStateWrite};
    use crate::background_launch::{
        BackgroundLaunchKind, BackgroundLaunchOptions, BackgroundLaunchSpec, LAUNCH_SPEC_VERSION,
    };
    use std::process::Command;

    fn shorts(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn argv_routes_rm_token_to_the_subcommand() {
        use crate::argv::Argv;
        use crate::commands::Commands;
        // `lingxi-cli rm <id>` resolves to the Rm subcommand with the positional
        // id (and reports the `rm` top-level name for the version gate).
        let a = Argv::from_iter(["lingxi-cli", "rm", "bc7c6b33"]).unwrap();
        let cmd = a.command.expect("rm token must parse as a subcommand");
        assert_eq!(cmd.top_level_name(), "rm");
        match cmd {
            Commands::Rm(c) => {
                assert_eq!(c.id.as_deref(), Some("bc7c6b33"));
                assert!(!c.help);
            }
            other => panic!("expected Rm, got {other:?}"),
        }
        // `rm -h` sets the manual help flag; no positional required.
        let a = Argv::from_iter(["lingxi-cli", "rm", "-h"]).unwrap();
        match a.command.unwrap() {
            Commands::Rm(c) => {
                assert!(c.help);
                assert!(c.id.is_none());
            }
            other => panic!("expected Rm, got {other:?}"),
        }
    }

    #[test]
    fn help_text_is_byte_exact() {
        // Verified against the 2.1.207 binary byte dump.
        assert_eq!(
            RM_HELP,
            "Usage: claude rm <id>\n\n  Delete a background session and its worktree. Unlike `stop`, works on already-exited sessions.\n"
        );
    }

    #[test]
    fn match_prefix_unique_none_ambiguous() {
        let s = shorts(&["aaaa1111", "aaab2222", "bbbb3333"]);
        // Exact id wins.
        assert_eq!(
            match_prefix(&s, "aaaa1111"),
            PrefixMatch::Unique("aaaa1111".to_string())
        );
        // Unique prefix.
        assert_eq!(
            match_prefix(&s, "bbbb"),
            PrefixMatch::Unique("bbbb3333".to_string())
        );
        // No match.
        assert_eq!(match_prefix(&s, "zzzz"), PrefixMatch::None);
        // Ambiguous prefix — matches preserve sorted order.
        assert_eq!(
            match_prefix(&s, "aaa"),
            PrefixMatch::Ambiguous(shorts(&["aaaa1111", "aaab2222"]))
        );
    }

    #[test]
    fn error_messages_are_byte_exact() {
        assert_eq!(
            no_job_message("dead"),
            "No job matching 'dead'. Run 'claude agents' to list running sessions."
        );
        assert_eq!(
            ambiguous_message("aaa", &shorts(&["aaaa1111", "aaab2222"])),
            "Ambiguous prefix 'aaa', matches: aaaa1111, aaab2222"
        );
        assert_eq!(
            couldnt_confirm_message("bc7c6b33"),
            "couldn't confirm bc7c6b33 was stopped \u{2014} the background service may be restarting. Try again in a moment."
        );
    }

    #[test]
    fn kept_reason_strings_match_binary_map() {
        // The binary `keptReason` map {dirty, branch_mismatch, live_lock,
        // remove_failed}.
        assert_eq!(KeptReason::Dirty.text(), "has uncommitted changes");
        assert_eq!(
            KeptReason::BranchMismatch.text(),
            "is on a different branch"
        );
        assert_eq!(
            KeptReason::LiveLock.text(),
            "is locked \u{2014} in use by another live session, or locked by hand"
        );
        assert_eq!(KeptReason::RemoveFailed.text(), "could not be removed");
    }

    #[test]
    fn removed_message_bare_and_kept_suffix() {
        assert_eq!(removed_message("bc7c6b33", None), "removed bc7c6b33");
        assert_eq!(
            removed_message("bc7c6b33", Some((KeptReason::RemoveFailed, "/w/tree"))),
            "removed bc7c6b33\n  worktree could not be removed \u{2014} kept at /w/tree"
        );
    }

    #[test]
    fn perform_delete_unlinks_the_job_state_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let respawn: Vec<String> = Vec::new();
        let job = JobStateWrite {
            state: "done",
            tempo: Some("idle"),
            name: None,
            session_id: Some("sid-1"),
            cwd: Some("/home/u/proj"),
            origin_cwd: Some("/home/u/proj"),
            created_at: Some("2026-07-04T00:00:00.000Z"),
            intent: Some("do the thing"),
            display_intent: None,
            template: Some("bg"),
            respawn_flags: &respawn,
            in_flight: None,
            backend: Some("daemon"),
            initial_prompt: Some("do the thing"),
            detail: None,
            worker_pid: None,
            worker_proc_start: None,
            phase: None,
            worker_generation: None,
            claim_token: None,
            claim_owner: None,
            claim_created_at: None,
            claim_lease_ms: None,
        };
        write_job_state(home, "bc7c6b33", &job).unwrap();
        let dir = jobs_dir(home).join("bc7c6b33");
        assert!(dir.exists());

        let stored = crate::agents_registry::read_job(home, "bc7c6b33").unwrap();
        let kept = perform_delete(home, "bc7c6b33", &stored, "cli").unwrap();
        // Plain project cwd → no managed worktree → nothing kept.
        assert!(kept.is_none());
        assert!(!dir.exists(), "jobs/<short> state dir is unlinked");
    }

    #[test]
    fn perform_delete_rejects_stale_snapshot_after_worker_generation_changes() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path().to_path_buf();
        let short = "fade0001";
        let flags: Vec<String> = Vec::new();
        write_job_state(
            &home,
            short,
            &crate::agents_registry::JobStateWrite {
                state: "working",
                tempo: Some("active"),
                name: None,
                session_id: Some("11111111-1111-1111-1111-111111111111"),
                cwd: Some("/work"),
                origin_cwd: Some("/work"),
                created_at: Some("2026-07-04T00:00:00.000Z"),
                intent: Some("intent"),
                display_intent: None,
                template: Some("bg"),
                respawn_flags: &flags,
                in_flight: None,
                backend: Some("daemon"),
                initial_prompt: None,
                detail: None,
                worker_pid: Some(4100),
                worker_proc_start: Some("OLD-START"),
                phase: Some("running"),
                worker_generation: Some("old-gen"),
                claim_token: None,
                claim_owner: None,
                claim_created_at: None,
                claim_lease_ms: None,
            },
        )
        .unwrap();
        let stale = crate::agents_registry::read_job(&home, short).unwrap();
        crate::agents_registry::patch_job_state_if_matches(
            &home,
            short,
            crate::agents_registry::JobStateMatch {
                state: "working",
                phase: Some("running"),
                worker_pid: Some(4100),
                worker_proc_start: Some("OLD-START"),
                worker_generation: Some("old-gen"),
                claim_token: None,
                claim_owner: None,
                claim_created_at: None,
                claim_lease_ms: None,
            },
            crate::agents_registry::JobStatePatch {
                state: Some("working"),
                tempo: None,
                cwd: None,
                detail: None,
                worker_pid: Some(Some(4200)),
                worker_proc_start: Some(Some("NEW-START")),
                phase: Some(Some("running")),
                worker_generation: Some(Some("new-gen")),
                claim_token: Some(None),
                claim_owner: Some(None),
                claim_created_at: Some(None),
                claim_lease_ms: Some(None),
            },
        )
        .unwrap();

        let err = perform_delete(&home, short, &stale, "cli").unwrap_err();
        assert!(err.contains("state changed") || err.contains("kill_unconfirmed"));
        assert!(crate::agents_registry::jobs_dir(&home).join(short).exists());
    }

    #[test]
    fn managed_worktree_detected_only_under_worktrees_dir() {
        use crate::agents_registry::JobState;
        let mut job = JobState {
            cwd: Some("/home/u/proj".to_string()),
            ..JobState::default()
        };
        assert!(managed_worktree_path(&job).is_none());
        job.cwd = Some(format!(
            "/home/u/proj/{}/worktrees/fix-thing",
            branding::DOT_DIR
        ));
        assert_eq!(
            managed_worktree_path(&job),
            Some(PathBuf::from(format!(
                "/home/u/proj/{}/worktrees/fix-thing",
                branding::DOT_DIR
            )))
        );
    }

    fn sample_worker(
        pid: i32,
        short: &str,
        worktree_path: &Path,
        token: &str,
    ) -> crate::daemon_roster::WorkerRecord {
        crate::daemon_roster::WorkerRecord {
            pid,
            proc_start: None,
            session_id: "11111111-1111-1111-1111-111111111111".to_string(),
            rendezvous_sock: "/tmp/rv.sock".to_string(),
            pty_sock: None,
            messaging_sock: None,
            cli_version: Some("2.1.207".to_string()),
            started_at: 1_700_000_000_000,
            attempt: 0,
            cwd: worktree_path.display().to_string(),
            worktree_path: None,
            dispatch: crate::daemon_roster::Dispatch {
                proto: crate::daemon_roster::PROTO,
                short: short.to_string(),
                nonce: None,
                session_id: "11111111-1111-1111-1111-111111111111".to_string(),
                created_at: 1_700_000_000_000,
                source: crate::daemon_roster::DispatchSource::Fleet,
                cwd: worktree_path.display().to_string(),
                launch: crate::daemon_roster::Launch::Prompt {
                    args: vec!["hello".to_string()],
                },
                launch_spec: None,
                env: std::collections::BTreeMap::new(),
                reattach_env: None,
                worktree: Some(crate::daemon_roster::Worktree {
                    path: worktree_path.display().to_string(),
                    ownership_token: token.to_string(),
                }),
                isolation: crate::daemon_roster::Isolation::Worktree,
                respawn_flags: Vec::new(),
                attach_stall_respawns: None,
                agent: None,
                routine: None,
                seed: None,
                cols: None,
                rows: None,
            },
            pending_respawn: None,
            dec_modes: None,
            rv_auth: None,
            pty_auth: None,
            extra: serde_json::Map::new(),
        }
    }

    fn git(dir: &Path, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "LingXi")
            .env("GIT_AUTHOR_EMAIL", "lingxi@example.com")
            .env("GIT_COMMITTER_NAME", "LingXi")
            .env("GIT_COMMITTER_EMAIL", "lingxi@example.com")
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed:\nstdout={}\nstderr={}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn seed_git_managed_worktree(
        write_token: bool,
    ) -> (
        tempfile::TempDir,
        PathBuf,
        String,
        crate::agents_registry::JobState,
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let repo = tmp.path().join("repo");
        let short = "cafe0001".to_string();
        let session_id = "11111111-1111-1111-1111-111111111111".to_string();
        let token = "token-123".to_string();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init"]);
        std::fs::write(repo.join("tracked.txt"), "base\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "init"]);

        let managed_root = repo.join(branding::DOT_DIR).join("worktrees");
        std::fs::create_dir_all(&managed_root).unwrap();
        let managed = managed_root.join("fix");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "worktree-fix",
                managed.to_str().unwrap(),
            ],
        );

        let state = JobStateWrite {
            state: "done",
            tempo: Some("idle"),
            name: None,
            session_id: Some(&session_id),
            cwd: Some(managed.to_str().unwrap()),
            origin_cwd: Some(repo.to_str().unwrap()),
            created_at: Some("2026-07-21T00:00:00.000Z"),
            intent: Some("clean up"),
            display_intent: None,
            template: Some("bg"),
            respawn_flags: &[] as &[String],
            in_flight: None,
            backend: Some("daemon"),
            initial_prompt: Some("clean up"),
            detail: None,
            worker_pid: None,
            worker_proc_start: None,
            phase: None,
            worker_generation: None,
            claim_token: None,
            claim_owner: None,
            claim_created_at: None,
            claim_lease_ms: None,
        };
        write_job_state(&home, &short, &state).unwrap();
        let spec = BackgroundLaunchSpec {
            schema_version: LAUNCH_SPEC_VERSION,
            short: short.clone(),
            created_at: 1,
            preflight_approved: true,
            launch: BackgroundLaunchKind::Fresh,
            session_id: session_id.clone(),
            transcript_path: home.join("session.jsonl").display().to_string(),
            cwd: managed.display().to_string(),
            origin_cwd: repo.display().to_string(),
            worktree_path: Some(managed.display().to_string()),
            worktree_ownership_token: write_token.then(|| token.clone()),
            initial_prompt: Some("clean up".to_string()),
            shell_handoff: Vec::new(),
            handoff: None,
            options: BackgroundLaunchOptions::default(),
            env: std::collections::BTreeMap::new(),
            terminal: crate::background_launch::TerminalSize::default(),
        };
        crate::background_launch::write_launch_spec(&home, &short, &spec).unwrap();
        if write_token {
            crate::daemon_roster::write_worktree_ownership_marker(
                &managed,
                &short,
                &session_id,
                &token,
            )
            .unwrap();
        }
        let job = crate::agents_registry::read_job(&home, &short).unwrap();
        (tmp, home, short, job)
    }

    #[test]
    fn managed_delete_rejects_missing_worktree_token() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let runtime_dir = home;
        let managed = home.join(branding::DOT_DIR).join("worktrees").join("fix");
        std::fs::create_dir_all(&managed).unwrap();

        let short = "aaaa1111";
        let mut roster = crate::daemon_roster::empty_roster(0);
        roster.workers.insert(
            short.to_string(),
            sample_worker(std::process::id() as i32, short, &managed, "token-123"),
        );
        crate::daemon_roster::write_roster(runtime_dir, &roster).unwrap();

        let job = crate::agents_registry::JobState {
            cwd: Some(managed.display().to_string()),
            ..Default::default()
        };

        let err = verify_managed_worktree_token(runtime_dir, short, &job).unwrap_err();
        assert!(err.contains("missing or unreadable ownership marker"));
    }

    #[test]
    fn managed_delete_rejects_missing_or_mismatched_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let runtime_dir = home;
        let managed = home.join(branding::DOT_DIR).join("worktrees").join("fix");
        std::fs::create_dir_all(&managed).unwrap();

        let short = "aaaa1111";
        let mut roster = crate::daemon_roster::empty_roster(0);
        roster.workers.insert(
            short.to_string(),
            sample_worker(std::process::id() as i32, short, &managed, "token-123"),
        );
        crate::daemon_roster::write_roster(runtime_dir, &roster).unwrap();

        let job = crate::agents_registry::JobState {
            cwd: Some(managed.display().to_string()),
            ..Default::default()
        };

        let marker = crate::daemon_roster::WorktreeOwnershipMarker {
            schema_version: crate::daemon_roster::WORKTREE_OWNERSHIP_MARKER_SCHEMA_VERSION,
            short: short.to_string(),
            session_id: "11111111-1111-1111-1111-111111111111".to_string(),
            ownership_token: "token-different".to_string(),
            canonical_worktree_path: std::fs::canonicalize(&managed)
                .unwrap()
                .display()
                .to_string(),
            created_at_millis: 1,
        };
        let marker_path = crate::daemon_roster::worktree_ownership_marker_path(&managed);
        std::fs::write(&marker_path, serde_json::to_vec_pretty(&marker).unwrap()).unwrap();

        let err = verify_managed_worktree_token(runtime_dir, short, &job).unwrap_err();
        assert_eq!(
            err,
            "Failed to verify managed-worktree deletion for aaaa1111: worktree ownership metadata mismatch."
        );
    }

    #[test]
    fn managed_delete_requires_matching_token_and_path() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let runtime_dir = home;
        let managed = home.join(branding::DOT_DIR).join("worktrees").join("fix");
        std::fs::create_dir_all(&managed).unwrap();

        let short = "aaaa1111";
        let mut roster = crate::daemon_roster::empty_roster(0);
        roster.workers.insert(
            short.to_string(),
            sample_worker(std::process::id() as i32, short, &managed, "token-123"),
        );
        crate::daemon_roster::write_roster(runtime_dir, &roster).unwrap();

        let marker = crate::daemon_roster::WorktreeOwnershipMarker {
            schema_version: crate::daemon_roster::WORKTREE_OWNERSHIP_MARKER_SCHEMA_VERSION,
            short: short.to_string(),
            session_id: "11111111-1111-1111-1111-111111111111".to_string(),
            ownership_token: "token-123".to_string(),
            canonical_worktree_path: std::fs::canonicalize(&managed)
                .unwrap()
                .display()
                .to_string(),
            created_at_millis: 1,
        };
        let marker_path = crate::daemon_roster::worktree_ownership_marker_path(&managed);
        std::fs::write(&marker_path, serde_json::to_vec_pretty(&marker).unwrap()).unwrap();

        let state = JobStateWrite {
            state: "done",
            tempo: Some("idle"),
            name: None,
            session_id: Some("sid-1"),
            cwd: Some(managed.to_str().unwrap()),
            origin_cwd: None,
            created_at: Some("2026-07-04T00:00:00.000Z"),
            intent: Some("do the thing"),
            display_intent: None,
            template: Some("bg"),
            respawn_flags: &[] as &[String],
            in_flight: None,
            backend: Some("daemon"),
            initial_prompt: Some("do the thing"),
            detail: None,
            worker_pid: None,
            worker_proc_start: None,
            phase: None,
            worker_generation: None,
            claim_token: None,
            claim_owner: None,
            claim_created_at: None,
            claim_lease_ms: None,
        };
        write_job_state(home, short, &state).unwrap();

        let job = crate::agents_registry::read_job(home, short).unwrap();
        assert!(verify_managed_worktree_token(runtime_dir, short, &job).is_ok());
    }

    #[test]
    fn perform_delete_removes_clean_managed_worktree() {
        let (_tmp, home, short, job) = seed_git_managed_worktree(true);
        let managed = managed_worktree_path(&job).unwrap();

        let kept = perform_delete(&home, &short, &job, "cli").unwrap();
        assert!(kept.is_none());
        assert!(!managed.exists(), "clean managed worktree is removed");
        assert!(
            !crate::agents_registry::jobs_dir(&home)
                .join(&short)
                .exists(),
            "job state is removed"
        );
    }

    #[test]
    fn perform_delete_keeps_dirty_managed_worktree() {
        let (_tmp, home, short, job) = seed_git_managed_worktree(true);
        let managed = managed_worktree_path(&job).unwrap();
        std::fs::write(managed.join("tracked.txt"), "dirty\n").unwrap();

        let kept = perform_delete(&home, &short, &job, "cli").unwrap();
        assert_eq!(
            kept,
            Some((
                KeptReason::Dirty,
                std::fs::canonicalize(&managed)
                    .unwrap()
                    .display()
                    .to_string()
            ))
        );
        assert!(managed.exists(), "dirty worktree must be preserved");
        assert!(
            !crate::agents_registry::jobs_dir(&home)
                .join(&short)
                .exists(),
            "job state is still removed"
        );
    }

    #[test]
    fn managed_delete_keeps_git_locked_worktree() {
        let (_tmp, home, short, job) = seed_git_managed_worktree(true);
        let managed = managed_worktree_path(&job).unwrap();
        let repo = managed
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .unwrap();
        git(
            repo,
            &[
                "worktree",
                "lock",
                "--reason",
                "test",
                managed.to_str().unwrap(),
            ],
        );

        let kept = perform_delete(&home, &short, &job, "cli").unwrap();
        assert_eq!(
            kept,
            Some((
                KeptReason::LiveLock,
                std::fs::canonicalize(&managed)
                    .unwrap()
                    .display()
                    .to_string()
            ))
        );
        assert!(managed.exists(), "locked worktree must be preserved");
    }

    #[test]
    fn lock_probe_failure_is_not_treated_as_unlocked() {
        let (_tmp, home, short, job) = seed_git_managed_worktree(true);
        let binding = resolve_verified_worktree_binding(&home, &short, &job)
            .unwrap()
            .expect("verified binding");
        let anchored = worktree_delete::open(&binding).unwrap();
        std::fs::remove_file(binding.canonical_path.join(".git")).unwrap();
        std::fs::rename(
            binding.repo_root.join(".git"),
            binding.repo_root.join(".git-disabled"),
        )
        .unwrap();

        assert_eq!(
            worktree_lock_status(&anchored),
            WorktreeLockStatus::CannotVerify
        );
    }

    #[test]
    fn perform_delete_rejects_unsafe_legacy_launch_metadata() {
        let (_tmp, home, short, job) = seed_git_managed_worktree(false);

        let err = perform_delete(&home, &short, &job, "cli").unwrap_err();
        assert_eq!(
            err,
            "Failed to verify managed-worktree deletion for cafe0001: unsafe legacy ownership metadata."
        );
        assert!(
            crate::agents_registry::jobs_dir(&home)
                .join(&short)
                .exists(),
            "unverified records must not be deleted"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_file_id_is_stable_across_reopen() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("worktree");
        std::fs::create_dir(&dir).unwrap();
        let first = worktree_delete::capture_identity(&dir).unwrap();
        let second = worktree_delete::capture_identity(&dir).unwrap();
        assert_eq!(first, second);
    }

    #[cfg(windows)]
    #[test]
    fn windows_reparse_points_are_rejected() {
        use std::os::windows::fs::symlink_dir;

        let tmp = tempfile::tempdir().unwrap();
        let victim = tmp.path().join("victim");
        let link = tmp.path().join("link");
        std::fs::create_dir(&victim).unwrap();
        if symlink_dir(&victim, &link).is_err() {
            return;
        }
        let err = worktree_delete::capture_identity(&link).unwrap_err();
        assert!(err.to_string().contains("reparse point"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_anchored_delete_rejects_reparse_replacement() {
        use std::os::windows::fs::symlink_dir;

        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(repo.join(branding::DOT_DIR).join("worktrees")).unwrap();
        let managed = repo.join(branding::DOT_DIR).join("worktrees").join("fix");
        std::fs::create_dir(&managed).unwrap();
        let binding = ManagedWorktreeBinding::new(
            "cafe0001",
            "11111111-1111-1111-1111-111111111111",
            &managed,
            "token-123",
        )
        .unwrap();
        let anchored = worktree_delete::open(&binding).unwrap();

        let moved = repo
            .join(branding::DOT_DIR)
            .join("worktrees")
            .join("fix-moved");
        let victim = repo.join("victim");
        std::fs::create_dir(&victim).unwrap();
        std::fs::rename(&managed, &moved).unwrap();
        if symlink_dir(&victim, &managed).is_err() {
            return;
        }

        let err = worktree_delete::remove(&binding, &anchored).unwrap_err();
        assert!(err.to_string().contains("identity changed"));
        assert!(moved.exists());
    }

    #[cfg(windows)]
    #[test]
    fn windows_git_probe_rejects_path_replacement() {
        let (_tmp, home, short, job) = seed_git_managed_worktree(true);
        let binding = resolve_verified_worktree_binding(&home, &short, &job)
            .unwrap()
            .expect("verified binding");
        let anchored = worktree_delete::open(&binding).unwrap();
        let original = binding.canonical_path.clone();
        let moved = original.with_file_name("fix-moved");
        std::fs::rename(&original, &moved).unwrap();
        std::fs::create_dir(&original).unwrap();

        let error = anchored.git_output(&["status", "--porcelain"]).unwrap_err();
        assert!(error.to_string().contains("identity changed"));
        assert!(moved.exists());
    }

    #[cfg(unix)]
    #[test]
    fn anchored_delete_rejects_directory_replacement() {
        let (_tmp, home, short, job) = seed_git_managed_worktree(true);
        let binding = resolve_verified_worktree_binding(&home, &short, &job)
            .unwrap()
            .expect("verified binding");
        let anchored = worktree_delete::open(&binding).unwrap();

        let original = binding.canonical_path.clone();
        let moved = binding
            .repo_root
            .join(branding::DOT_DIR)
            .join("worktrees")
            .join("fix-moved");
        std::fs::rename(&original, &moved).unwrap();
        std::fs::create_dir(&original).unwrap();
        let sentinel = original.join("intruder.txt");
        std::fs::write(&sentinel, "leave me").unwrap();

        let err = worktree_delete::remove(&binding, &anchored).unwrap_err();
        assert!(err.to_string().contains("identity changed"));
        assert!(
            sentinel.exists(),
            "replacement directory must survive failed anchored deletion"
        );
        assert!(moved.exists());
    }

    #[cfg(unix)]
    #[test]
    fn anchored_delete_rejects_symlink_replacement() {
        use std::os::unix::fs::symlink;

        let (_tmp, home, short, job) = seed_git_managed_worktree(true);
        let binding = resolve_verified_worktree_binding(&home, &short, &job)
            .unwrap()
            .expect("verified binding");
        let anchored = worktree_delete::open(&binding).unwrap();

        let original = binding.canonical_path.clone();
        let moved = binding
            .repo_root
            .join(branding::DOT_DIR)
            .join("worktrees")
            .join("fix-moved");
        let victim = binding.repo_root.join("victim-dir");
        std::fs::create_dir(&victim).unwrap();
        let victim_file = victim.join("sentinel.txt");
        std::fs::write(&victim_file, "protected").unwrap();

        std::fs::rename(&original, &moved).unwrap();
        symlink(&victim, &original).unwrap();

        let err = worktree_delete::remove(&binding, &anchored).unwrap_err();
        assert!(err.to_string().contains("identity changed"));
        assert!(
            std::fs::symlink_metadata(&original)
                .unwrap()
                .file_type()
                .is_symlink(),
            "replacement symlink must survive failed anchored deletion"
        );
        assert_eq!(std::fs::read_to_string(&victim_file).unwrap(), "protected");
        assert!(moved.exists());
    }
}
