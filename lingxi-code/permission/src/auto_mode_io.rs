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
//! RESIDUAL: [`rg_files`] spawns `rg` off `PATH`. claude-code ships an embedded
//! ripgrep and resolves it through the logic modelled in
//! `tools/file/src/ripgrep_mode.rs`, whose own docs record that the system-`rg`
//! subprocess backend is not implemented here yet. Where no `rg` resolves, the
//! oracle's `RPo` also returns an empty list (`catch { return [] }`), so this
//! matches its behaviour — but note the consequence: the glob-backed sections
//! then render as "nothing found" rather than as "could not look". Wiring this
//! to the embedded binary is what makes those sections trustworthy.

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
fn run_capped(mut cmd: Command, timeout: Duration) -> Option<(i32, String, bool)> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = Vec::new();
        let mut stdout = stdout;
        let _ = stdout.read_to_end(&mut buf);
        buf
    });

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if started.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    let partial = reader.join().unwrap_or_default();
                    return Some((-1, String::from_utf8_lossy(&partial).into_owned(), true));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => {
                let _ = reader.join();
                return None;
            }
        }
    };

    let bytes = reader.join().ok()?;
    Some((
        status.code().unwrap_or(-1),
        String::from_utf8_lossy(&bytes).into_owned(),
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
/// `.git` and `node_modules` are excluded, the walk is depth-limited, and the
/// results are filtered by `keep` before being capped. Any failure (including a
/// missing `rg`) yields an empty list rather than an error, matching the
/// oracle's `catch { return [] }`.
#[must_use]
pub fn rg_files(
    root: &Path,
    patterns: &[&str],
    limit: usize,
    depth: usize,
    keep: Option<&regex::Regex>,
) -> Vec<String> {
    let mut cmd = Command::new("rg");
    cmd.current_dir(root);
    cmd.args([
        "--files",
        "--hidden",
        "--max-depth",
        &depth.to_string(),
        "-g",
        "!.git",
        "-g",
        "!node_modules",
    ]);
    for p in patterns {
        cmd.args(["-g", p]);
    }
    let Some((_, out, _)) = run_capped(cmd, recon_timeout()) else {
        return Vec::new();
    };
    // `zsy` — make the paths relative to the root.
    let root_prefix = format!("{}/", root.display());
    let listed: Vec<String> = out
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| {
            l.strip_prefix(&root_prefix)
                .unwrap_or(l)
                .replace('\\', "/")
        })
        .filter(|l| keep.is_none_or(|re| re.is_match(l)))
        .collect();
    finalize_glob(listed, limit)
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
            BUCKET_SCAN_DISTINCT_CAP, BUCKET_SCAN_GLOBS, BUCKET_SCAN_MAX_FILESIZE,
            BUCKET_SCAN_TIMEOUT_MS, FLAGGED_LIST_CAP,
        };

        let mut cmd = Command::new("rg");
        cmd.current_dir(&self.root);
        cmd.args([
            "-o",
            "-H",
            "--no-line-number",
            "--no-messages",
            "--no-heading",
            "--color=never",
            "--null",
            "--hidden",
            "-g",
            "!.git",
            "-g",
            "!node_modules",
        ]);
        for g in BUCKET_SCAN_GLOBS {
            cmd.args(["-g", g]);
        }
        cmd.args([
            "--max-filesize",
            BUCKET_SCAN_MAX_FILESIZE,
            "-e",
            "[a-z0-9.+-]?(s3|gs|az)://[a-z0-9][a-z0-9._-]*",
        ]);

        // A scan that could not start at all is a FAILURE, not an empty result.
        let (code, out, timed_out) =
            run_capped(cmd, Duration::from_millis(BUCKET_SCAN_TIMEOUT_MS))?;

        struct Tally {
            occurrences: usize,
            files: usize,
            last_file: String,
        }
        let mut tallies: std::collections::HashMap<String, Tally> =
            std::collections::HashMap::new();
        let mut hit_cap = false;

        'lines: for line in out.split('\n') {
            let Some(nul) = line.find('\0') else { continue };
            let (path, rest) = (&line[..nul], &line[nul + 1..]);
            for name in extract_bucket_names(rest) {
                if name.len() > 256 {
                    continue;
                }
                if let Some(t) = tallies.get_mut(&name) {
                    t.occurrences += 1;
                    if t.last_file != path {
                        t.files += 1;
                        t.last_file = path.to_string();
                    }
                } else {
                    if tallies.len() >= BUCKET_SCAN_DISTINCT_CAP {
                        hit_cap = true;
                        break 'lines;
                    }
                    tallies.insert(
                        name,
                        Tally {
                            occurrences: 1,
                            files: 1,
                            last_file: path.to_string(),
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
            // `rg` exit 2 is a real error; exit 1 just means no matches.
            truncated: hit_cap || timed_out || code == 2,
        })
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
        fn config_scans(&self) -> Result<String, ()> {
            Ok(crate::auto_mode_producers::config_scans_section(
                &FsConfigScanSource::new(&self.root),
            ))
        }
        fn default_labels(&self) -> Result<String, ()> {
            Ok(crate::auto_mode_producers::default_labels_section())
        }
    }

    /// Can a `rg` actually be spawned here? See the RESIDUAL in the module
    /// docs — the glob-backed scans are inert without one.
    fn ripgrep_available() -> bool {
        Command::new("rg")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
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

        // Only the one ungated producer that is not ported yet degrades; the
        // five that ARE ported all produced real content.
        assert_eq!(
            block.failed_sections,
            vec!["Recent usage in this project (names only)"]
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
        // ...and the glob-backed scans, where a ripgrep can be spawned.
        if ripgrep_available() {
            assert!(block.text.contains("#### Makefile/justfile targets"));
            assert!(block.text.contains("- deploy"));
        }
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
