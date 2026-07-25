//! WIZARD-06 — the recon gather's real I/O (2.1.220).
//!
//! The producers in [`crate::auto_mode_producers`] are pure; this module is
//! where they actually touch the disk and spawn subprocesses. Keeping the split
//! means the rendering logic is tested without a filesystem, and the containment
//! rules live in exactly one place.
//!
//! Everything here reads a repository that may be hostile, so three rules hold
//! throughout:
//!
//! * **Never follow a symlink.** Reads use `O_NOFOLLOW`, and
//!   [`contained_read`] additionally requires the canonicalised target to be
//!   exactly `root/relative` — so an intermediate symlinked directory is
//!   refused even though each individual component resolved.
//! * **Never let the repo run code.** Every `git` invocation carries
//!   [`crate::auto_mode_producers::GIT_HARDENING_FLAGS`], which disables hooks,
//!   the filesystem monitor and the credential prompt.
//! * **Never block forever.** Subprocesses are killed at
//!   [`crate::auto_mode_producers::SUBPROCESS_TIMEOUT_MS`], and reads are capped.
//!
//! The glob-backed scans run IN-PROCESS on the `ignore` crate — claude-code's
//! own "embedded ripgrep" mode, and what `tools/file`'s Grep already does here.
//! Spawning `rg` instead would make every glob-backed recon section depend on a
//! binary being on `PATH`, and a missing binary renders "nothing found" rather
//! than "could not look" — the exact confusion the rest of this subsystem
//! works to prevent.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::auto_mode_producers::{
    DocSource, LocalSettingsSource, RepoFactsSource, SettingsReconSource, DOC_GLOB_LIMIT,
    DOC_GLOB_MAX_DEPTH, DOC_READ_CAP_CLAUDE_MD, GIT_HARDENING_FLAGS,
    SUBPROCESS_TIMEOUT_MS,
};

/// `s$s` — the longest path `eNd` will keep.
const MAX_GLOB_PATH_LEN: usize = 256;

/// `jIe`'s truncation suffix. Distinct from `Z1d`'s: this one reports the
/// character cut AND the file's real size, so a truncated read can never pass
/// for a whole one.
fn truncated_suffix(cap: usize, size: u64) -> String {
    format!("\n\u{2026}[truncated at {cap} chars of {size} bytes]")
}

/// `jIe` — open with `O_NOFOLLOW`, require a regular file, read up to `cap`
/// characters, and mark the result when it was cut.
///
/// Returns `None` for anything that is not a readable regular file, including a
/// symlinked final component.
#[must_use]
pub fn secure_read_capped(path: &Path, cap: usize, require_nlink1: bool) -> Option<String> {
    use std::io::Read;

    #[cfg(unix)]
    let opened = {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
    };
    #[cfg(not(unix))]
    let opened = match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => return None,
        Ok(_) => std::fs::File::open(path),
        Err(_) => return None,
    };

    let file = opened.ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() {
        return None;
    }
    #[cfg(unix)]
    if require_nlink1 {
        use std::os::unix::fs::MetadataExt;
        if meta.nlink() != 1 {
            return None;
        }
    }
    #[cfg(not(unix))]
    let _ = require_nlink1;

    let size = meta.len();
    // `Math.min(size, cap + 1)` — one past the cap is enough to detect a cut.
    let want = size.min(cap as u64 + 1);
    let mut bytes = Vec::new();
    (&file).take(want).read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes).into_owned();

    // The cut is by CHARACTERS, not bytes, and never splits one.
    if text.chars().count() > cap {
        let head: String = text.chars().take(cap).collect();
        return Some(format!("{head}{}", truncated_suffix(cap, size)));
    }
    Some(text)
}

/// `jhr` — read `relative` from inside `root`, refusing any indirection.
///
/// Both paths are canonicalised; the target must land strictly under the root
/// AND be exactly `root/relative`. That second check is what rejects a
/// symlinked intermediate directory: each component resolves fine, but the
/// resolved path is no longer the one that was asked for.
#[must_use]
pub fn contained_read(root: &Path, relative: &str, cap: usize) -> Option<String> {
    let root_c = std::fs::canonicalize(root).ok()?;
    let target_c = std::fs::canonicalize(root.join(relative)).ok()?;
    let rel = target_c.strip_prefix(&root_c).ok()?;
    if rel.as_os_str().is_empty() {
        return None;
    }
    if target_c != root_c.join(relative) {
        return None;
    }
    secure_read_capped(&target_c, cap, false)
}

/// Does any component of `root/relative` not exist, or is any of them a symlink?
///
/// `None` when a component could not be stat'd at all (which the caller reads
/// as "absent"), `Some(true)` when one is a symlink.
#[must_use]
pub fn path_symlink_check(root: &Path, relative: &str) -> Option<bool> {
    let mut at = root.to_path_buf();
    for part in relative.split('/') {
        at.push(part);
        let meta = std::fs::symlink_metadata(&at).ok()?;
        if meta.file_type().is_symlink() {
            return Some(true);
        }
    }
    Some(false)
}

/// Run a command, killing it at `timeout`. `None` only when it could not be
/// spawned at all.
///
/// On timeout the child is killed and whatever it had already written is
/// returned with `timed_out = true` — a streaming scan needs its partial
/// results AND the fact that they are partial.
///
/// stdout is drained on its own thread: an 8 MB pipe would otherwise fill and
/// deadlock the child while we sat polling for its exit.
fn run_capped(cmd: Command, timeout: Duration) -> Option<(i32, String, bool)> {
    run_capped_full(cmd, timeout).map(|(c, out, _err, t)| (c, out, t))
}

/// As [`run_capped`], but also returns stderr — `gh`'s availability check reads it.
fn run_capped_full(
    mut cmd: Command,
    timeout: Duration,
) -> Option<(i32, String, String, bool)> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;

    use std::io::Read;
    let drain = |mut pipe: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(p) = pipe.as_mut() {
                let _ = p.read_to_end(&mut buf);
            }
            buf
        })
    };
    let stdout: Option<Box<dyn std::io::Read + Send>> =
        child.stdout.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>);
    let stderr: Option<Box<dyn std::io::Read + Send>> =
        child.stderr.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>);
    let reader = drain(stdout);
    let err_reader = drain(stderr);

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if started.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    let partial = reader.join().unwrap_or_default();
                    let errs = err_reader.join().unwrap_or_default();
                    return Some((
                        -1,
                        String::from_utf8_lossy(&partial).into_owned(),
                        String::from_utf8_lossy(&errs).into_owned(),
                        true,
                    ));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => {
                let _ = reader.join();
                let _ = err_reader.join();
                return None;
            }
        }
    };

    let bytes = reader.join().ok()?;
    let errs = err_reader.join().unwrap_or_default();
    Some((
        status.code().unwrap_or(-1),
        String::from_utf8_lossy(&bytes).into_owned(),
        String::from_utf8_lossy(&errs).into_owned(),
        false,
    ))
}

fn recon_timeout() -> Duration {
    Duration::from_millis(SUBPROCESS_TIMEOUT_MS)
}

/// `hFt` — `git -C <root> <hardening flags> <args>`; stdout with one trailing
/// newline removed, or `""` on any failure or non-zero exit.
#[must_use]
pub fn git_output(root: &Path, args: &[&str]) -> String {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(root);
    cmd.args(GIT_HARDENING_FLAGS);
    cmd.args(args);
    match run_capped(cmd, recon_timeout()) {
        Some((0, out, false)) => out.strip_suffix('\n').unwrap_or(&out).to_string(),
        _ => String::new(),
    }
}

/// `Vsy` — how many lines the command produced; `0` unless it exited cleanly
/// with output.
#[must_use]
pub fn git_line_count(root: &Path, args: &[&str]) -> usize {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(root);
    cmd.args(GIT_HARDENING_FLAGS);
    cmd.args(args);
    match run_capped(cmd, recon_timeout()) {
        Some((0, out, false)) if !out.is_empty() => out.matches('\n').count(),
        _ => 0,
    }
}

/// `eNd` — de-duplicate, drop over-long paths, sort, cap.
fn finalize_glob(mut paths: Vec<String>, limit: usize) -> Vec<String> {
    paths.retain(|p| p.len() <= MAX_GLOB_PATH_LEN);
    paths.sort_unstable();
    paths.dedup();
    paths.truncate(limit);
    paths
}

/// `RPo` — list files under `root` matching `patterns`.
///
/// Runs IN-PROCESS on the `ignore` crate rather than spawning `rg`. That is
/// claude-code's own "embedded ripgrep" mode, and it is what `tools/file`'s
/// Grep already does here — `ignore` is the same library ripgrep is built on.
/// Spawning instead would make every glob-backed recon section depend on a
/// binary being on `PATH`, and when it is not there the section renders
/// "nothing found" rather than "could not look", which is precisely the
/// confusion the rest of this subsystem works to avoid.
///
/// `.git` and `node_modules` are excluded, the walk is depth-limited, hidden
/// files are included, and `.gitignore` is respected — matching
/// `rg --files --hidden --max-depth N -g '!.git' -g '!node_modules'`.
#[must_use]
pub fn rg_files(
    root: &Path,
    patterns: &[&str],
    limit: usize,
    depth: usize,
    keep: Option<&regex::Regex>,
) -> Vec<String> {
    let Some(overrides) = build_overrides(root, patterns) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .max_depth(Some(depth))
        .overrides(overrides)
        .build();
    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(root) else {
            continue;
        };
        let rel = rel.to_string_lossy().replace('\\', "/");
        if keep.is_none_or(|re| re.is_match(&rel)) {
            out.push(rel);
        }
    }
    finalize_glob(out, limit)
}

/// Build the `-g` override set: the two standing exclusions plus `patterns`.
fn build_overrides(root: &Path, patterns: &[&str]) -> Option<ignore::overrides::Override> {
    let mut builder = ignore::overrides::OverrideBuilder::new(root);
    builder.add("!.git").ok()?;
    builder.add("!node_modules").ok()?;
    for p in patterns {
        builder.add(p).ok()?;
    }
    builder.build().ok()
}

// ── network paths ────────────────────────────────────────────────────────────

/// `tu` — a UNC path (`\\server\share` or `//server/share`).
fn is_unc(path: &str) -> bool {
    let mut chars = path.chars();
    matches!(chars.next(), Some('/' | '\\')) && matches!(chars.next(), Some('/' | '\\'))
}

/// `em` — a WSL path (`\\wsl$\…`, `\\wsl.localhost\…`).
///
/// UNC-SHAPED but local, which is why it is carved out of [`is_network_path`].
fn is_wsl_path(path: &str) -> bool {
    if !is_unc(path) {
        return false;
    }
    let rest = &path[2..];
    let lower = rest.to_lowercase();
    let after = if let Some(a) = lower.strip_prefix("wsl$") {
        a
    } else if let Some(a) = lower.strip_prefix("wsl.localhost") {
        a
    } else {
        return false;
    };
    after.starts_with('/') || after.starts_with('\\')
}

/// `vEi`/`d_e` — an autofs `/net/<host>/…` path.
fn is_autofs_net(path: &str) -> bool {
    if !path.starts_with('/') {
        return false;
    }
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            parts.pop();
            continue;
        }
        parts.push(part);
        if parts.len() == 2 && parts[0].to_lowercase() == "net" {
            return true;
        }
    }
    false
}

/// `K7e` — would touching this path reach a NETWORK host?
///
/// A UNC share or an autofs `/net/<host>` mount is not merely slow: merely
/// stat-ing one authenticates to, or at minimum resolves, the named host. The
/// recon refuses to walk such a path at all rather than attempting it and
/// reporting the result — see
/// [`crate::auto_mode_gates::HOME_REPOS_NETWORK_HOME`].
///
/// WSL paths are UNC-shaped but local, so they are explicitly not network.
#[must_use]
pub fn is_network_path(path: &str) -> bool {
    (is_unc(path) && !is_wsl_path(path)) || is_autofs_net(path)
}

// ── tail reads ───────────────────────────────────────────────────────────────

/// The outcome of a tail read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TailRead {
    /// The file is not there — skip it silently.
    Absent,
    /// It exists but could not be read; the caller marks the gather PARTIAL
    /// rather than pretending the file held nothing.
    Unreadable,
    /// The last `cap` bytes, and whether the head was cut off.
    Read {
        /// The decoded content.
        content: String,
        /// The file was longer than `cap`.
        truncated: bool,
    },
}

/// `jIe`'s `fromTail` branch — read the LAST `cap` bytes of a file.
///
/// Shell history grows at the end, so the recent commands are the tail. When
/// the read starts mid-file the first partial line is dropped, so a command is
/// never reported with its head sliced off.
#[must_use]
pub fn secure_read_tail(path: &Path, cap: u64, require_nlink1: bool) -> TailRead {
    use std::io::{Read, Seek, SeekFrom};

    #[cfg(unix)]
    let opened = {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
    };
    #[cfg(not(unix))]
    let opened = match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => return TailRead::Unreadable,
        Ok(_) => std::fs::File::open(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return TailRead::Absent,
        Err(_) => return TailRead::Unreadable,
    };

    let mut file = match opened {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return TailRead::Absent,
        Err(_) => return TailRead::Unreadable,
    };
    let Ok(meta) = file.metadata() else {
        return TailRead::Unreadable;
    };
    if !meta.is_file() {
        return TailRead::Unreadable;
    }
    #[cfg(unix)]
    if require_nlink1 {
        use std::os::unix::fs::MetadataExt;
        if meta.nlink() != 1 {
            return TailRead::Unreadable;
        }
    }
    #[cfg(not(unix))]
    let _ = require_nlink1;

    let size = meta.len();
    let from = size.saturating_sub(cap);
    if file.seek(SeekFrom::Start(from)).is_err() {
        return TailRead::Unreadable;
    }
    let mut bytes = Vec::new();
    if file.take(size - from).read_to_end(&mut bytes).is_err() {
        return TailRead::Unreadable;
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();

    if from == 0 {
        return TailRead::Read {
            content: text,
            truncated: false,
        };
    }
    // Started mid-file: drop the partial first line.
    let content = match text.find('\n') {
        Some(i) => text[i + 1..].to_string(),
        None => String::new(),
    };
    TailRead::Read {
        content,
        truncated: true,
    }
}

// ── read-deny across path aliases ────────────────────────────────────────────

/// `esy` — re-express `path` as if it sat under `to` instead of `from`.
///
/// `None` when `path` is not under `from` at all. Comparison is
/// case-insensitive on Windows, matching the platform's own path semantics.
#[must_use]
pub fn rebase_path(path: &str, from: &str, to: &str, windows: bool) -> Option<String> {
    let norm = |s: &str| {
        if windows {
            s.to_lowercase()
        } else {
            s.to_string()
        }
    };
    let (p, f) = (norm(path), norm(from));
    if p == f {
        return Some(to.to_string());
    }
    let sep = if windows { '\\' } else { '/' };
    let prefix = if f.ends_with(sep) {
        f
    } else {
        format!("{f}{sep}")
    };
    if !p.starts_with(&prefix) {
        return None;
    }
    Some(format!("{to}{}", &path[from.len()..]))
}

/// `Smt` — is `path` read-denied under EITHER spelling of a directory that has
/// two names?
///
/// A home directory routinely has two: the one the environment reports and the
/// one `realpath` resolves it to (`/home/u` vs `/mnt/data/u`, `/var/...` vs
/// `/private/var/...`). A `permissions.deny` rule is written against one of
/// them. Checking only the path as given would let the other spelling walk
/// straight past a rule the user wrote — so the path is rebased between the two
/// and the deny predicate is asked about each.
///
/// `is_denied` is injected because the deny set lives with the session policy,
/// not with the gatherer.
#[must_use]
pub fn denied_under_either_alias(
    path: &str,
    dir_a: &str,
    dir_b: &str,
    is_denied: &dyn Fn(&str) -> bool,
    windows: bool,
) -> bool {
    if is_denied(path) {
        return true;
    }
    if dir_a == dir_b {
        return false;
    }
    for (from, to) in [(dir_a, dir_b), (dir_b, dir_a)] {
        if let Some(alias) = rebase_path(path, from, to, windows) {
            if is_denied(&alias) {
                return true;
            }
        }
    }
    false
}

// ── concrete sources ─────────────────────────────────────────────────────────

/// Real-filesystem [`DocSource`] rooted at a project directory.
pub struct FsDocSource {
    root: PathBuf,
    user_config_dir: PathBuf,
    doc_glob: regex::Regex,
}

impl FsDocSource {
    /// Build a source for `root`, with the user's config directory for the
    /// user-level `CLAUDE.md`.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>, user_config_dir: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            user_config_dir: user_config_dir.into(),
            // `^\.claude\/(skills|rules|agents)\/`
            doc_glob: regex::Regex::new(r"^\.claude/(skills|rules|agents)/")
                .expect("static regex"),
        }
    }
}

impl DocSource for FsDocSource {
    fn user_claude_md(&self) -> Option<String> {
        secure_read_capped(
            &self.user_config_dir.join("CLAUDE.md"),
            DOC_READ_CAP_CLAUDE_MD,
            false,
        )
    }
    fn project_file(&self, relative: &str, cap: usize) -> Option<String> {
        contained_read(&self.root, relative, cap)
    }
    fn claude_doc_paths(&self) -> Vec<String> {
        rg_files(
            &self.root,
            &["SKILL.md", "*.md"],
            DOC_GLOB_LIMIT,
            DOC_GLOB_MAX_DEPTH,
            Some(&self.doc_glob),
        )
    }
}

/// Real-filesystem [`RepoFactsSource`].
pub struct FsRepoFactsSource {
    root: PathBuf,
}

impl FsRepoFactsSource {
    /// Build a source for the repository at `root`.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl RepoFactsSource for FsRepoFactsSource {
    fn git(&self, args: &[&str]) -> String {
        git_output(&self.root, args)
    }
    fn git_line_count(&self, args: &[&str]) -> usize {
        git_line_count(&self.root, args)
    }
    fn path_has_symlink_component(&self, relative: &str) -> Option<bool> {
        path_symlink_check(&self.root, relative)
    }
    fn read_file(&self, relative: &str, cap: usize) -> Option<String> {
        contained_read(&self.root, relative, cap)
    }
    fn repo_path(&self) -> String {
        self.root.display().to_string()
    }
}

/// Real-filesystem [`crate::auto_mode_producers::ConfigScanSource`].
pub struct FsConfigScanSource {
    root: PathBuf,
}

impl FsConfigScanSource {
    /// Build a source for the repository at `root`.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl crate::auto_mode_producers::ConfigScanSource for FsConfigScanSource {
    fn scan_files(&self, globs: &[&str], path_filter: Option<&regex::Regex>) -> Vec<String> {
        use crate::auto_mode_producers::{
            CONFIG_SCAN_FILE_LIMIT, CONFIG_SCAN_READ_CAP, DOC_GLOB_MAX_DEPTH,
        };
        rg_files(
            &self.root,
            globs,
            CONFIG_SCAN_FILE_LIMIT,
            DOC_GLOB_MAX_DEPTH,
            path_filter,
        )
        .into_iter()
        .filter_map(|p| contained_read(&self.root, &p, CONFIG_SCAN_READ_CAP))
        .collect()
    }

    fn list_paths(
        &self,
        globs: &[&str],
        limit: usize,
        depth: usize,
        filter: Option<&regex::Regex>,
    ) -> Vec<String> {
        rg_files(&self.root, globs, limit, depth, filter)
    }

    fn package_json(&self) -> Option<String> {
        contained_read(
            &self.root,
            "package.json",
            crate::auto_mode_producers::PACKAGE_JSON_READ_CAP,
        )
    }

    fn bucket_scan(&self) -> Option<crate::auto_mode_producers::BucketScan> {
        use crate::auto_mode_producers::{
            bucket_prefix_clusters, extract_bucket_names, BucketCount, BucketScan,
            BUCKET_SCAN_DISTINCT_CAP, BUCKET_SCAN_GLOBS, BUCKET_SCAN_MAX_FILESIZE_BYTES,
            BUCKET_SCAN_TIMEOUT_MS, DOC_GLOB_MAX_DEPTH, FLAGGED_LIST_CAP,
        };

        // In-process, like `rg_files` — see the note there.
        let overrides = build_overrides(&self.root, &BUCKET_SCAN_GLOBS)?;
        let deadline = Instant::now() + Duration::from_millis(BUCKET_SCAN_TIMEOUT_MS);

        struct Tally {
            occurrences: usize,
            files: usize,
            last_file: String,
        }
        let mut tallies: std::collections::HashMap<String, Tally> =
            std::collections::HashMap::new();
        let mut hit_cap = false;
        let mut timed_out = false;

        let walker = ignore::WalkBuilder::new(&self.root)
            .hidden(false)
            .max_depth(Some(DOC_GLOB_MAX_DEPTH))
            .overrides(overrides)
            .build();

        'files: for entry in walker.flatten() {
            if Instant::now() >= deadline {
                timed_out = true;
                break;
            }
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            // `--max-filesize`: an oversized config file is skipped, not read.
            if entry
                .metadata()
                .is_ok_and(|m| m.len() > BUCKET_SCAN_MAX_FILESIZE_BYTES)
            {
                continue;
            }
            let Ok(rel) = entry.path().strip_prefix(&self.root) else {
                continue;
            };
            let path = rel.to_string_lossy().replace('\\', "/");
            let Ok(content) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            for name in extract_bucket_names(&content) {
                if name.len() > 256 {
                    continue;
                }
                if let Some(t) = tallies.get_mut(&name) {
                    t.occurrences += 1;
                    if t.last_file != path {
                        t.files += 1;
                        t.last_file = path.clone();
                    }
                } else {
                    if tallies.len() >= BUCKET_SCAN_DISTINCT_CAP {
                        hit_cap = true;
                        break 'files;
                    }
                    tallies.insert(
                        name,
                        Tally {
                            occurrences: 1,
                            files: 1,
                            last_file: path.clone(),
                        },
                    );
                }
            }
        }

        let mut top: Vec<(String, BucketCount)> = tallies
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    BucketCount {
                        occurrences: v.occurrences,
                        files: v.files,
                    },
                )
            })
            .collect();
        top.sort_by(|a, b| {
            b.1.occurrences
                .cmp(&a.1.occurrences)
                .then_with(|| a.0.cmp(&b.0))
        });
        let distinct = top.len();
        let names: Vec<String> = top.iter().map(|(k, _)| k.clone()).collect();
        top.truncate(FLAGGED_LIST_CAP);

        Some(BucketScan {
            top,
            distinct,
            clusters: bucket_prefix_clusters(names.iter()),
            truncated: hit_cap || timed_out,
        })
    }
}

/// Real-filesystem [`crate::auto_mode_producers::ProjectUsageSource`].
pub struct FsProjectUsageSource {
    transcript_dir: PathBuf,
}

impl FsProjectUsageSource {
    /// Build a source for this project's transcript directory.
    #[must_use]
    pub fn new(transcript_dir: impl Into<PathBuf>) -> Self {
        Self {
            transcript_dir: transcript_dir.into(),
        }
    }
}

impl crate::auto_mode_producers::ProjectUsageSource for FsProjectUsageSource {
    fn transcripts(&self) -> Option<Vec<crate::auto_mode_producers::TranscriptFile>> {
        use crate::auto_mode_producers::{TranscriptFile, TRANSCRIPT_FILE_LIMIT};
        // An unreadable directory is "no history", matching the oracle's catch.
        let entries = std::fs::read_dir(&self.transcript_dir).ok()?;
        let mut found: Vec<(std::time::SystemTime, TranscriptFile)> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
            found.push((
                mtime,
                TranscriptFile {
                    path,
                    size: meta.len(),
                },
            ));
        }
        // Newest first, then capped -- so a busy project's OLD transcripts are
        // what falls off, not its recent ones.
        found.sort_by(|a, b| b.0.cmp(&a.0));
        found.truncate(TRANSCRIPT_FILE_LIMIT);
        Some(found.into_iter().map(|(_, f)| f).collect())
    }

    fn read_transcript(&self, path: &Path) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }
}

/// Real-filesystem [`LocalSettingsSource`] for `.claude/settings.local.json`.
pub struct FsLocalSettingsSource {
    root: PathBuf,
}

impl FsLocalSettingsSource {
    /// Build a source for the project at `root`.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
    fn dir(&self) -> PathBuf {
        self.root.join(".claude")
    }
    fn file(&self) -> PathBuf {
        self.dir().join("settings.local.json")
    }
}

impl LocalSettingsSource for FsLocalSettingsSource {
    fn claude_dir(&self) -> Option<bool> {
        let meta = std::fs::symlink_metadata(self.dir()).ok()?;
        Some(meta.is_dir())
    }
    fn local_file(&self) -> Option<(bool, u64, u64)> {
        let meta = std::fs::symlink_metadata(self.file()).ok()?;
        #[cfg(unix)]
        let nlink = {
            use std::os::unix::fs::MetadataExt;
            meta.nlink()
        };
        #[cfg(not(unix))]
        let nlink = 1u64;
        Some((meta.is_file(), nlink, meta.len()))
    }
    fn read_local(&self) -> Option<String> {
        secure_read_capped(
            &self.file(),
            crate::auto_mode_producers::SETTINGS_READ_CAP,
            true,
        )
    }
    fn tracked_in_git(&self) -> bool {
        !git_output(
            &self.root,
            &["ls-files", "--", ".claude/settings.local.json"],
        )
        .is_empty()
    }
}

/// Real-filesystem [`SettingsReconSource`].
pub struct FsSettingsReconSource {
    settings_path: PathBuf,
    local_block: String,
    classify_all_shell: bool,
}

impl FsSettingsReconSource {
    /// Build a source for `settings_path`, with the already-rendered
    /// project-local sub-block and the `classifyAllShell` flag.
    #[must_use]
    pub fn new(
        settings_path: impl Into<PathBuf>,
        local_block: String,
        classify_all_shell: bool,
    ) -> Self {
        Self {
            settings_path: settings_path.into(),
            local_block,
            classify_all_shell,
        }
    }
}

impl SettingsReconSource for FsSettingsReconSource {
    fn user_settings(&self) -> Result<Option<String>, ()> {
        match std::fs::symlink_metadata(&self.settings_path) {
            // Absent is fine; anything else present-but-unreadable is not.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(()),
            Ok(_) => secure_read_capped(
                &self.settings_path,
                crate::auto_mode_producers::SETTINGS_READ_CAP,
                false,
            )
            .map(Some)
            .ok_or(()),
        }
    }
    fn local_block(&self) -> String {
        self.local_block.clone()
    }
    fn classify_all_shell(&self) -> bool {
        self.classify_all_shell
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(dir: &Path, rel: &str, body: &str) -> PathBuf {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        p
    }

    #[test]
    fn a_capped_read_announces_its_truncation() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(dir.path(), "big.txt", &"x".repeat(100));
        let got = secure_read_capped(&p, 10, false).unwrap();
        assert!(got.starts_with(&"x".repeat(10)));
        assert!(got.contains("\u{2026}[truncated at 10 chars of 100 bytes]"));
        // Under the cap, no marker.
        assert_eq!(secure_read_capped(&p, 1000, false).unwrap(), "x".repeat(100));
    }

    #[test]
    fn the_cap_is_a_byte_window_but_the_marker_is_character_counted() {
        // The oracle reads `min(size, cap + 1)` BYTES and then compares the
        // DECODED length against the cap. For ASCII those coincide, so the
        // marker fires. For multi-byte content the byte window yields fewer
        // characters than the cap, the comparison never trips, and the read
        // comes back short WITHOUT a marker. That asymmetry is the oracle's;
        // reproducing it keeps the block byte-identical, and it is safe in the
        // direction that matters -- the model sees less than the file, never
        // more, and never a partial character.
        let dir = tempfile::tempdir().unwrap();
        let p = write(dir.path(), "u.txt", &"路".repeat(20));
        let got = secure_read_capped(&p, 5, false).unwrap();
        // 6 bytes read -> 2 whole characters, no marker.
        assert_eq!(got, "路".repeat(2));
        assert!(!got.contains("truncated"));
        // Whatever comes back is always valid, whole characters.
        assert!(got.chars().all(|c| c == '路'));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "real.txt", "secret");
        let link = dir.path().join("link.txt");
        std::os::unix::fs::symlink(dir.path().join("real.txt"), &link).unwrap();
        assert_eq!(secure_read_capped(&link, 100, false), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_hardlinked_file_is_refused_when_nlink_is_checked() {
        let dir = tempfile::tempdir().unwrap();
        let real = write(dir.path(), "real.txt", "secret");
        let alias = dir.path().join("alias.txt");
        std::fs::hard_link(&real, &alias).unwrap();
        assert_eq!(secure_read_capped(&alias, 100, true), None);
        // ...but allowed when the caller does not require it.
        assert_eq!(secure_read_capped(&alias, 100, false).as_deref(), Some("secret"));
    }

    #[test]
    fn a_contained_read_stays_inside_the_root() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "inside.txt", "ok");
        write(dir.path(), "sub/deep.txt", "deep");
        assert_eq!(
            contained_read(dir.path(), "inside.txt", 100).as_deref(),
            Some("ok")
        );
        assert_eq!(
            contained_read(dir.path(), "sub/deep.txt", 100).as_deref(),
            Some("deep")
        );
        // Escapes are refused.
        assert_eq!(contained_read(dir.path(), "../outside.txt", 100), None);
        assert_eq!(contained_read(dir.path(), "missing.txt", 100), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_intermediate_directory_is_refused() {
        // Each component resolves, but the resolved path is no longer the one
        // that was asked for -- which is exactly the case the equality check
        // exists to catch.
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write(outside.path(), "loot.txt", "secret");
        std::os::unix::fs::symlink(outside.path(), dir.path().join("docs")).unwrap();
        assert_eq!(contained_read(dir.path(), "docs/loot.txt", 100), None);
    }

    #[cfg(unix)]
    #[test]
    fn the_symlink_component_check_reports_each_case() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a/b.txt", "x");
        std::os::unix::fs::symlink(dir.path().join("a"), dir.path().join("link")).unwrap();
        assert_eq!(path_symlink_check(dir.path(), "a/b.txt"), Some(false));
        assert_eq!(path_symlink_check(dir.path(), "link/b.txt"), Some(true));
        assert_eq!(path_symlink_check(dir.path(), "nope/x.txt"), None);
    }

    #[test]
    fn a_hanging_subprocess_is_killed_at_the_timeout() {
        let mut cmd = Command::new("sleep");
        cmd.arg("30");
        let started = Instant::now();
        let (_, _, timed_out) = run_capped(cmd, Duration::from_millis(300)).unwrap();
        assert!(timed_out, "must report that it was cut short");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "must not wait for the child"
        );
    }

    #[test]
    fn git_output_is_empty_for_a_non_repo_and_hardened_for_a_real_one() {
        let dir = tempfile::tempdir().unwrap();
        // Not a repository -> non-zero exit -> empty, never an error.
        assert_eq!(git_output(dir.path(), &["remote"]), "");
        assert_eq!(git_line_count(dir.path(), &["ls-files"]), 0);

        // A real repo: the hardening flags must not break ordinary use.
        if Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .status()
            .is_ok_and(|s| s.success())
        {
            write(dir.path(), "f.txt", "hi");
            let _ = Command::new("git")
                .args(["add", "f.txt"])
                .current_dir(dir.path())
                .status();
            assert_eq!(git_line_count(dir.path(), &["ls-files"]), 1);
        }
    }

    #[test]
    fn settings_recon_distinguishes_absent_from_unreadable() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("settings.json");
        let src = FsSettingsReconSource::new(&missing, String::new(), false);
        assert_eq!(src.user_settings(), Ok(None), "absent is not an error");

        let present = write(dir.path(), "s.json", "{}");
        let src = FsSettingsReconSource::new(&present, String::new(), false);
        assert_eq!(src.user_settings(), Ok(Some("{}".to_string())));

        // Present but a symlink -> the no-follow read fails -> Err, which the
        // producer turns into "settings file present but unreadable".
        #[cfg(unix)]
        {
            let link = dir.path().join("link.json");
            std::os::unix::fs::symlink(&present, &link).unwrap();
            let src = FsSettingsReconSource::new(&link, String::new(), false);
            assert_eq!(src.user_settings(), Err(()));
        }
    }

    /// Wires the four ported producers to real filesystem sources.
    struct RealProducers {
        root: PathBuf,
        config: PathBuf,
    }
    impl crate::auto_mode_pregather::ReconProducers for RealProducers {
        fn project_docs(&self) -> Result<String, ()> {
            Ok(crate::auto_mode_producers::project_docs_section(
                &FsDocSource::new(&self.root, &self.config),
            ))
        }
        fn repo_facts(&self) -> Result<String, ()> {
            Ok(crate::auto_mode_producers::repo_facts_section(&FsRepoFactsSource::new(&self.root))
                .body)
        }
        fn existing_settings(&self) -> Result<String, ()> {
            let (local, _) = crate::auto_mode_producers::local_settings_block(
                &FsLocalSettingsSource::new(&self.root),
            );
            crate::auto_mode_producers::existing_settings_section(&FsSettingsReconSource::new(
                self.config.join("settings.json"),
                local,
                false,
            ))
        }
        fn project_usage(&self) -> Result<String, ()> {
            Ok(crate::auto_mode_producers::project_usage_section(
                &FsProjectUsageSource::new(self.root.join(".transcripts")),
            ))
        }
        fn config_scans(&self) -> Result<String, ()> {
            Ok(crate::auto_mode_producers::config_scans_section(
                &FsConfigScanSource::new(&self.root),
            ))
        }
        fn default_labels(&self) -> Result<String, ()> {
            Ok(crate::auto_mode_producers::default_labels_section())
        }
    }

    #[test]
    fn the_four_ported_producers_run_against_a_real_repository() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        let config = dir.path().join("config");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&config).unwrap();

        write(&root, "CLAUDE.md", "project rules");
        write(&root, ".gitignore", "target/\n.env\nMY_TOKEN\n");
        write(&root, "Makefile", "build:\n\tcargo build\ndeploy:\n\techo go\n");
        write(
            &root,
            "package.json",
            r#"{"scripts":{"build":"tsc","test":"vitest"}}"#,
        );
        write(
            &root,
            "deploy.yaml",
            "logs: s3://acme-logs/x\ndata: gs://acme-data/y\nagain: s3://acme-logs/z\n",
        );
        // A real transcript for the project-usage producer to mine.
        write(
            &root,
            ".transcripts/session.jsonl",
            &[
                serde_json::json!({"message":{"content":[
                    {"type":"tool_use","name":"Bash","input":{"command":"terraform apply -token=SECRETVALUE"}}
                ]}}).to_string(),
                serde_json::json!({"message":{"content":[
                    {"type":"tool_use","name":"Bash","input":{"command":"curl https://api.acme.io/v1"}}
                ]}}).to_string(),
            ]
            .join("\n"),
        );
        write(&config, "CLAUDE.md", "user rules");
        write(
            &config,
            "settings.json",
            &serde_json::json!({
                "permissions": { "allow": ["Bash(*)", "Bash(rm -rf *)", "Read(src/**)"] }
            })
            .to_string(),
        );
        write(
            &root,
            ".claude/settings.local.json",
            &serde_json::json!({ "autoMode": { "allow": ["Bash(x:*)"] } }).to_string(),
        );

        let producers = RealProducers {
            root: root.clone(),
            config: config.clone(),
        };
        let block = crate::auto_mode_pregather::build_recon_block(
            crate::auto_mode_pregather::GatherOptions::default(),
            &producers,
        );

        // Every ungated producer is ported now, so nothing degrades.
        assert!(
            block.failed_sections.is_empty(),
            "unexpected failures: {:?}",
            block.failed_sections
        );
        // ...docs read off the real disk,
        assert!(block.text.contains("#### ~/.lingxi/CLAUDE.md"));
        assert!(block.text.contains("\"user rules\""));
        assert!(block.text.contains("\"project rules\""));
        // ...repo facts ran git and reported the sensitive gitignore lines,
        assert!(block.text.contains("Repo path: "));
        assert!(block.text.contains("- `.env`"));
        assert!(block.text.contains("- `MY_TOKEN`"));
        assert!(!block.text.contains("target/"));
        // ...the two flagged lists split correctly off the real settings file,
        assert!(block.text.contains("- `Bash(*)`"));
        assert!(block.text.contains("- `Bash(rm -rf *)`"));
        assert!(!block.text.contains("Read(src/**)"));
        // ...the project-local autoMode keys were reported as found content,
        assert!(block.text.contains("NOT pre-approved config"));
        assert!(block.text.contains("Bash(x:*)"));
        // ...config scans read package.json directly (no glob needed),
        assert!(block.text.contains("#### package.json scripts"));
        assert!(block.text.contains("- build"));
        assert!(block.text.contains("- test"));
        // ...and the glob-backed scans, which need no external binary.
        assert!(block.text.contains("#### Makefile/justfile targets"));
        assert!(block.text.contains("- build"));
        assert!(block.text.contains("- deploy"));
        // The repo-wide bucket scan found the names in the config file.
        assert!(block.text.contains("#### Bucket names in config"));
        assert!(block.text.contains("- acme-logs"));
        assert!(block.text.contains("- acme-data"));
        // ...and the transcript miner reported NAMES ONLY.
        assert!(block.text.contains("Transcripts scanned: 1; Bash commands seen: 2"));
        assert!(block.text.contains("- terraform (1\u{d7})"));
        assert!(block.text.contains("- api.acme.io (1\u{d7})"));
        assert!(
            !block.text.contains("SECRETVALUE"),
            "a secret in a command line must never reach the block"
        );
        // ...and the shipped default labels are listed.
        assert!(block.text.contains("#### Default allow labels"));
        assert!(block.text.contains("- Read-Only Operations"));

        // The five gated producers were withheld without being run.
        assert_eq!(block.gated_sections.len(), 5);
        assert!(block.text.contains("_NOT GATHERED"));
        // Every one of the eleven sections is present either way.
        for title in crate::auto_mode_pregather::SECTION_TITLES {
            assert!(block.text.contains(title), "missing section: {title}");
        }
    }

    #[test]
    fn network_paths_are_recognised_so_they_are_never_touched() {
        // Merely stat-ing one authenticates to, or resolves, the named host.
        for p in [
            r"\\server\share\x",
            "//server/share/x",
            "/net/fileserver/home/u",
            "/net/host",
        ] {
            assert!(is_network_path(p), "{p} must be treated as network");
        }
        // WSL is UNC-SHAPED but local.
        for p in [r"\\wsl$\Ubuntu\home\u", r"\\wsl.localhost\Ubuntu\home\u"] {
            assert!(!is_network_path(p), "{p} is local");
        }
        // Ordinary local paths.
        for p in ["/home/u", "/Users/u", r"C:\Users\u", "/netlify/x", "/net"] {
            assert!(!is_network_path(p), "{p} is local");
        }
        // `..` cannot be used to sneak into `/net/<host>`.
        assert!(is_network_path("/a/../net/host/x"));
    }

    #[test]
    fn a_tail_read_returns_the_end_and_drops_the_partial_first_line() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(dir.path(), "hist", "one\ntwo\nthree\nfour\n");

        // Whole file fits: nothing dropped, not truncated.
        assert_eq!(
            secure_read_tail(&p, 1000, false),
            TailRead::Read {
                content: "one\ntwo\nthree\nfour\n".to_string(),
                truncated: false
            }
        );

        // Only the tail fits: the sliced-open first line is dropped, so no
        // command is ever reported with its head cut off.
        let TailRead::Read { content, truncated } = secure_read_tail(&p, 12, false) else {
            panic!("expected a read");
        };
        assert!(truncated);
        assert_eq!(content, "three\nfour\n");
        assert!(!content.contains("wo\n"));
    }

    #[test]
    fn a_tail_read_distinguishes_absent_from_unreadable() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            secure_read_tail(&dir.path().join("nope"), 100, false),
            TailRead::Absent
        );
        // A directory is not a readable history file.
        assert_eq!(secure_read_tail(dir.path(), 100, false), TailRead::Unreadable);
        // A symlink is refused, and that is UNREADABLE rather than absent --
        // the caller must mark the gather partial, not assume nothing was there.
        #[cfg(unix)]
        {
            write(dir.path(), "real", "x");
            let link = dir.path().join("link");
            std::os::unix::fs::symlink(dir.path().join("real"), &link).unwrap();
            assert_eq!(secure_read_tail(&link, 100, false), TailRead::Unreadable);
        }
    }

    #[test]
    fn rebasing_moves_a_path_between_two_spellings_of_a_directory() {
        assert_eq!(
            rebase_path("/home/u/.ssh/id_rsa", "/home/u", "/mnt/data/u", false).as_deref(),
            Some("/mnt/data/u/.ssh/id_rsa")
        );
        // The directory itself rebases to the other directory.
        assert_eq!(
            rebase_path("/home/u", "/home/u", "/mnt/data/u", false).as_deref(),
            Some("/mnt/data/u")
        );
        // Not under `from` at all.
        assert_eq!(rebase_path("/etc/passwd", "/home/u", "/mnt/data/u", false), None);
        // A sibling whose name merely starts the same is NOT under it.
        assert_eq!(rebase_path("/home/user2/x", "/home/u", "/mnt/u", false), None);
        // Windows compares case-insensitively.
        assert_eq!(
            rebase_path(r"C:\Users\U\x", r"c:\users\u", r"D:\alt", true).as_deref(),
            Some(r"D:\alt\x")
        );
    }

    #[test]
    fn a_deny_rule_blocks_both_spellings_of_a_home_directory() {
        // The rule is written against the realpath'd spelling; the walk hands
        // over the environment's spelling. Checking only what it was handed
        // would walk straight past the user's own rule.
        let denied = |p: &str| p.starts_with("/mnt/data/u/.ssh");
        assert!(denied_under_either_alias(
            "/home/u/.ssh/id_rsa",
            "/home/u",
            "/mnt/data/u",
            &denied,
            false
        ));
        // ...and symmetrically, a rule against the environment spelling blocks
        // the realpath'd one.
        let denied = |p: &str| p.starts_with("/home/u/.ssh");
        assert!(denied_under_either_alias(
            "/mnt/data/u/.ssh/id_rsa",
            "/home/u",
            "/mnt/data/u",
            &denied,
            false
        ));
        // An unrelated path is not denied by either.
        assert!(!denied_under_either_alias(
            "/home/u/work/README.md",
            "/home/u",
            "/mnt/data/u",
            &denied,
            false
        ));
        // When the two spellings coincide, only the direct check applies.
        let denied = |p: &str| p == "/home/u/secret";
        assert!(denied_under_either_alias(
            "/home/u/secret",
            "/home/u",
            "/home/u",
            &denied,
            false
        ));
        assert!(!denied_under_either_alias(
            "/home/u/ok",
            "/home/u",
            "/home/u",
            &denied,
            false
        ));
    }

    #[test]
    fn glob_results_are_deduped_sorted_and_capped() {
        let paths = vec![
            "b.md".to_string(),
            "a.md".to_string(),
            "b.md".to_string(),
            "x".repeat(300),
            "c.md".to_string(),
        ];
        assert_eq!(finalize_glob(paths, 2), vec!["a.md", "b.md"]);
    }

    #[test]
    fn the_local_settings_source_reports_lstat_facts() {
        let dir = tempfile::tempdir().unwrap();
        let src = FsLocalSettingsSource::new(dir.path());
        // Nothing there at all.
        assert_eq!(src.claude_dir(), None);

        std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
        assert_eq!(src.claude_dir(), Some(true));
        assert_eq!(src.local_file(), None);

        write(dir.path(), ".claude/settings.local.json", "{\"autoMode\":{}}");
        let (is_file, nlink, size) = src.local_file().unwrap();
        assert!(is_file);
        assert_eq!(nlink, 1);
        assert_eq!(size, 15);

        // `.claude` as a symlink is reported as not-a-directory, so the gate
        // ladder refuses without probing behind it.
        #[cfg(unix)]
        {
            let dir2 = tempfile::tempdir().unwrap();
            std::os::unix::fs::symlink(dir.path().join(".claude"), dir2.path().join(".claude"))
                .unwrap();
            let src2 = FsLocalSettingsSource::new(dir2.path());
            assert_eq!(src2.claude_dir(), Some(false));
        }
    }
}
