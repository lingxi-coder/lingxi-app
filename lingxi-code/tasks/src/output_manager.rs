//! Sandboxed spool-file manager for task stdout/stderr.
//!
//! See spec §6.6 / D8 — task output is materialized as files under a
//! sandbox directory, with a per-file and total byte budget.

use platform_api::FileSystem;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::Mutex;

/// Disk cap for a single task's output file. Mirrors claude-code's
/// `MAX_TASK_OUTPUT_BYTES = 5 * 1024 * 1024 * 1024` (`diskOutput.ts:30`).
/// Past this, [`TaskOutputManager::append`] drops further chunks and writes a
/// single truncation marker, matching `DiskTaskOutput.append`.
pub const MAX_TASK_OUTPUT_BYTES: u64 = 5 * 1024 * 1024 * 1024;

/// The cap's unit: JS `String.length`, i.e. UTF-16 code units. A CJK character
/// is 3 UTF-8 bytes but 1 unit, and an emoji is 4 bytes but 2 — counting bytes
/// tripped the cap early for any non-ASCII spool.
fn utf16_units(content: &str) -> u64 {
    content.chars().map(|c| c.len_utf16() as u64).sum()
}

/// Display string for [`MAX_TASK_OUTPUT_BYTES`] used in the truncation marker
/// (claude-code `MAX_TASK_OUTPUT_BYTES_DISPLAY = '5GB'`, `diskOutput.ts:31`).
pub const MAX_TASK_OUTPUT_BYTES_DISPLAY: &str = "5GB";

/// claude-code `wt` — the one thing a failed write still tries to get onto disk,
/// so a reader can tell truncation from silence. Leading AND trailing newline.
const OUTPUT_OMITTED_MARKER: &str = "\n[output omitted: it could not be written to disk]\n";

/// claude-code `Bt = 8388608` — `getTaskOutput` never returns more than the
/// LAST 8 MiB of a spool, however large the file is
/// (`Rbt(t, e = Bt)` → `k_(handle, e)`).
///
/// Distinct from [`MAX_TASK_OUTPUT_BYTES`], which is the 5GB WRITE cap: that
/// one stops the spool growing, this one stops a full read from materialising
/// gigabytes into memory on the way to a caller that will truncate it to tens
/// of thousands of characters anyway.
pub const MAX_TASK_OUTPUT_READ_BYTES: u64 = 8 * 1024 * 1024;

/// Keep at most the last [`MAX_TASK_OUTPUT_READ_BYTES`] and announce what was
/// dropped, claude-code `Rbt`:
///
/// ```js
/// let{content:r,bytesTotal:s,bytesRead:c}=o;
/// if(s>c)return`[${Math.round((s-c)/1024)}KB of earlier output omitted]\n${r}`;
/// return r
/// ```
///
/// The oracle seeks, so its `bytesRead` is exact; this trims an
/// already-materialised string, so the cut is snapped FORWARD to a UTF-8
/// boundary and the omitted count is recomputed from what actually survived —
/// the header must describe the real cut, not the requested one.
fn apply_read_tail_cap(content: String) -> String {
    let total = content.len() as u64;
    if total <= MAX_TASK_OUTPUT_READ_BYTES {
        return content;
    }
    let mut cut = (total - MAX_TASK_OUTPUT_READ_BYTES) as usize;
    while cut < content.len() && !content.is_char_boundary(cut) {
        cut += 1;
    }
    let omitted = cut as f64 / 1024.0;
    format!(
        "[{}KB of earlier output omitted]\n{}",
        omitted.round() as u64,
        &content[cut..]
    )
}

/// Owner of the task-output sandbox directory.
pub struct TaskOutputManager {
    output_dir: PathBuf,
    fs: Arc<dyn FileSystem>,
    /// Identity of the output directory observed by this manager. The
    /// platform rooted operations still perform handle-relative I/O; this
    /// lightweight pin catches a directory that was moved and replaced
    /// between calls before a new rooted handle is opened.
    root_pin: Mutex<OutputRootState>,
    /// Per-spool bytes-written counter + capped flag, keyed by spool path.
    /// Backs the write-side 5GB disk cap ([`MAX_TASK_OUTPUT_BYTES`]): once a
    /// spool crosses the cap its entry is marked capped and further appends are
    /// dropped (after a single truncation marker is written), mirroring
    /// claude-code's `DiskTaskOutput.#bytesWritten` / `#capped`.
    caps: Mutex<HashMap<PathBuf, CapState>>,
    /// Per-spool serialization for append/terminal replacement operations. A
    /// terminal Local App result must replace its raw payload as one
    /// indivisible write; reads also take this lock so a fail-closed terminal
    /// override cannot race a stale filesystem read. Unrelated task spools
    /// remain independent.
    write_locks: Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>,
    /// Spools whose terminal payload has been replaced. Further handler
    /// appends are ignored so a late raw chunk cannot follow the canonical
    /// result back into the externally readable spool.
    terminal: Mutex<HashSet<PathBuf>>,
    /// Authoritative terminal payloads. These are recorded before attempting
    /// the best-effort on-disk rewrite, so a failed rewrite cannot leave stale
    /// bytes readable through this manager or advertise them as canonical.
    terminal_overrides: Mutex<HashMap<PathBuf, TerminalOverride>>,
}

#[derive(Debug, Clone)]
struct TerminalOverride {
    content: String,
    physical_spool_authoritative: bool,
}

/// Per-spool write-side cap state (claude-code `DiskTaskOutput` `#bytesWritten`
/// + `#capped`).
#[derive(Debug, Default, Clone, Copy)]
struct CapState {
    /// Running size in UTF-16 CODE UNITS — the unit claude-code's
    /// `DiskTaskOutput.append` accumulates (`this.#f += t.length` over JS
    /// strings), not bytes on disk.
    bytes_written: u64,
    capped: bool,
    /// claude-code `lostOutput` — a write for this spool has failed at least
    /// once, so its contents are known-incomplete. See
    /// [`TaskOutputManager::lost_output`]; set-once, never cleared.
    lost_output: bool,
}

/// Errors produced by [`TaskOutputManager`].
#[derive(Debug, Clone, Error)]
pub enum OutputError {
    /// I/O failure forwarded from the [`FileSystem`] trait.
    #[error("io: {0}")]
    Io(String),
    /// Refusal to write to a path that escapes the sandbox root.
    #[error("path escapes output dir: {0}")]
    PathEscape(String),
    /// A spool file already exists for this id — the exclusive (`O_EXCL`)
    /// create refused to clobber it. Guards the double-allocate truncate race:
    /// a second [`TaskOutputManager::allocate`] of the same id errors here
    /// instead of silently truncating output a worker already appended.
    #[error("spool already allocated: {0}")]
    AlreadyExists(String),
    /// The output directory no longer names the directory owned by this
    /// manager, or an unsafe symlink was introduced. The recovery guidance is
    /// intentionally explicit about removing the link itself, never its target.
    #[error("{0}")]
    SwapRefused(String),
}

#[derive(Debug, Default)]
struct OutputRootState {
    initialized: bool,
    identity: Option<platform_api::rooted_fs::RootIdentity>,
}

/// Read options for [`TaskOutputManager::read`].
#[derive(Debug, Clone, Default)]
pub struct OutputOptions {
    /// Byte offset to start reading from.
    pub offset: Option<u64>,
    /// Maximum number of bytes to return.
    pub limit: Option<u64>,
}

/// Output payload returned by [`TaskOutputManager::read`].
#[derive(Debug, Clone)]
pub struct TaskOutput {
    /// Raw text content (already trimmed by `limit`).
    pub content: String,
    /// Total line count of the authoritative output projection.
    pub total_lines: u64,
    /// True if `content` is a prefix of the authoritative output projection.
    pub truncated: bool,
    /// Whether the physical spool contains the same authoritative bytes.
    pub physical_spool_authoritative: bool,
}

impl TaskOutputManager {
    /// Construct a manager rooted at `output_dir`.
    #[must_use]
    pub fn new(output_dir: PathBuf, fs: Arc<dyn FileSystem>) -> Self {
        Self {
            output_dir,
            fs,
            root_pin: Mutex::new(OutputRootState::default()),
            caps: Mutex::new(HashMap::new()),
            write_locks: Mutex::new(HashMap::new()),
            terminal: Mutex::new(HashSet::new()),
            terminal_overrides: Mutex::new(HashMap::new()),
        }
    }

    /// The absolute spool directory this manager owns.
    #[must_use]
    pub fn output_dir(&self) -> &Path {
        &self.output_dir
    }

    /// Return the spool path for `task_id` WITHOUT creating the file. Refuses
    /// any `..` or absolute leak (D8).
    ///
    /// Use this to recover the path a handler already allocated (the spool path
    /// is a deterministic function of the id), so the registry never calls
    /// [`allocate`](Self::allocate) a second time for the same id — the
    /// double-allocate truncate race fix.
    pub fn path_for(&self, task_id: &str) -> Result<PathBuf, OutputError> {
        // Extension `.output` byte-aligns with claude-code's
        // `getTaskOutputPath` (`diskOutput.ts:72-74` → `${taskId}.output`).
        let filename = format!("{task_id}.output");
        let path = self.output_dir.join(&filename);
        if !path.starts_with(&self.output_dir) {
            return Err(OutputError::PathEscape(filename));
        }
        Ok(path)
    }

    fn relative_path_for(&self, output_file: &Path) -> Result<PathBuf, OutputError> {
        let relative = output_file
            .strip_prefix(&self.output_dir)
            .map_err(|_| OutputError::PathEscape(output_file.display().to_string()))?
            .to_path_buf();
        platform_api::rooted_fs::validate_relative_path(&relative)
            .map_err(|_| OutputError::PathEscape(relative.display().to_string()))?;
        Ok(relative)
    }

    /// claude-code `b(path, reason, recovery?)`:
    ///
    /// ```js
    /// let o=`task output swap refused (${e}): ${t}`+(i===void 0?"":`. To recover: ${i}.`)
    /// ```
    ///
    /// The recovery clause is OPTIONAL, and the oracle attaches it to exactly
    /// ONE of its eight refusals — `tasks dir moved or linked`, the only one a
    /// user can act on. The port appended it to every reason, so a refusal that
    /// says a single output FILE changed identity underneath us told the user to
    /// restart with a fresh temp directory, which does not address that at all.
    fn swap_refused(&self, reason: &str) -> OutputError {
        OutputError::SwapRefused(format!(
            "task output swap refused ({reason}): {}",
            self.output_dir.display(),
        ))
    }

    /// The one refusal that carries the oracle's recovery clause. `LINGXI_TMPDIR`
    /// is the port's spelling of `CLAUDE_CODE_TMPDIR`; naming it is the point of
    /// the sentence — "its temporary-directory setting" left the reader with
    /// nothing to set. The oracle strips trailing separators from the temp root
    /// before interpolating it.
    fn swap_refused_dir_moved(&self) -> OutputError {
        let root = self.output_dir.display().to_string();
        let root = root.trim_end_matches(['/', '\\']);
        OutputError::SwapRefused(format!(
            "task output swap refused (tasks dir moved or linked): {root}. To recover: restart with LINGXI_TMPDIR set to a fresh directory; or, if {root} is a stray directory or a symbolic link that should not be there, remove that entry itself (not what it points to) and restart.",
        ))
    }

    async fn check_output_root(
        &self,
    ) -> Result<Option<platform_api::rooted_fs::RootIdentity>, OutputError> {
        let mut state = self.root_pin.lock().await;
        let current = match self.fs.root_identity_no_follow(&self.output_dir).await {
            Ok(identity) => identity,
            Err(error) if state.initialized => {
                // The pin no longer resolves — the directory was replaced by a
                // symlink or removed under us. Same actionable refusal as an
                // identity CHANGE, and the same one the oracle raises when its
                // own pin is refused; it logs the cause separately rather than
                // folding it into the message, which is why the errno-carrying
                // `lstat refused a swapped path (${a})` family is per-FILE.
                tracing::warn!("task output: pin of {} refused: {error}", self.output_dir.display());
                return Err(self.swap_refused_dir_moved());
            }
            Err(error) => return Err(self.map_rooted_error(error)),
        };
        if state.initialized {
            if state.identity != current {
                // The one actionable refusal — the tasks directory itself was
                // replaced. This is the oracle's `tasks dir moved or linked`.
                return Err(self.swap_refused_dir_moved());
            }
        } else {
            state.initialized = true;
            state.identity = current;
        }
        Ok(state.identity)
    }

    fn map_rooted_error(&self, error: platform_api::FsError) -> OutputError {
        match error {
            platform_api::FsError::OutsideWorkspace(_) => {
                // A LingXi-specific check (the port validates the relative path
                // before opening, where the oracle relies on `O_NOFOLLOW`), so
                // the reason stays truthful to what was refused rather than
                // borrowing an oracle string that describes an open() failure.
                self.swap_refused("a parent or final path component is unsafe")
            }
            other => OutputError::Io(other.to_string()),
        }
    }

    async fn write_lock_for(&self, output_file: &Path) -> Arc<Mutex<()>> {
        let mut locks = self.write_locks.lock().await;
        locks
            .entry(output_file.to_path_buf())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Allocate a fresh spool file inside `output_dir`. Refuse any `..` or
    /// absolute leak (D8).
    ///
    /// The file is created with `O_CREAT | O_EXCL | O_NOFOLLOW`
    /// ([`FileSystem::create_new_file`]), byte-aligned with claude-code's
    /// `initTaskOutput`:
    /// - Exclusive create makes allocation non-destructive — a second
    ///   `allocate` for the same id returns [`OutputError::AlreadyExists`]
    ///   rather than truncating output a worker already appended (T4).
    /// - `O_NOFOLLOW` refuses a pre-planted symlink at the spool path, closing
    ///   the symlink-follow write vector (T18).
    pub async fn allocate(&self, task_id: &str) -> Result<PathBuf, OutputError> {
        let path = self.path_for(task_id)?;
        let relative = self.relative_path_for(&path)?;
        let root_identity = self.check_output_root().await?;
        self.fs
            .create_new_file_rooted_no_follow_pinned(
                &self.output_dir,
                &relative,
                root_identity.as_ref(),
            )
            .await
            .map_err(|e| match e {
                platform_api::FsError::AlreadyExists(p) => OutputError::AlreadyExists(p),
                other => self.map_rooted_error(other),
            })?;
        Ok(path)
    }

    /// Delete a spool that turned out to be unused.
    ///
    /// claude-code deletes a shell's task-output file when it is redundant —
    /// the command finished in the foreground and its output was returned
    /// inline, so nothing will ever read the file (`deleteOutputFile`, guarded
    /// by `outputFileRedundant`). Without this, minting one output file per
    /// Bash call would leave a file behind for every command in the session.
    ///
    /// Best-effort and idempotent: a spool that is already gone is not an error.
    ///
    /// # Errors
    /// Returns [`OutputError::PathEscape`] when the path is not inside this
    /// manager's output directory.
    pub async fn discard(&self, output_file: &Path) -> Result<(), OutputError> {
        let relative = self.relative_path_for(output_file)?;
        let write_lock = self.write_lock_for(output_file).await;
        let _writes = write_lock.lock().await;
        self.caps.lock().await.remove(output_file);
        self.terminal.lock().await.remove(output_file);
        self.terminal_overrides.lock().await.remove(output_file);
        match self
            .fs
            .delete_file_rooted_no_follow(&self.output_dir, &relative)
            .await
        {
            Ok(()) | Err(platform_api::FsError::NotFound(_)) => Ok(()),
            Err(other) => Err(self.map_rooted_error(other)),
        }
    }

    /// Append a chunk to a task's spool, enforcing the per-file 5GB disk cap
    /// ([`MAX_TASK_OUTPUT_BYTES`]) on the WRITE side.
    ///
    /// Aligned with claude-code's `DiskTaskOutput.append`: the running count is
    /// `this.#f += t.length` where `t` is a JS STRING, so the unit is UTF-16
    /// CODE UNITS, not bytes — upstream re-measures real bytes only when it
    /// builds the Buffer to write. Counting UTF-8 bytes here tripped the cap
    /// early for any non-ASCII spool (a CJK character is 3 bytes but 1 unit).
    /// Once the count would cross the cap the spool is marked capped —
    /// a single truncation marker
    /// `\n[output truncated: exceeded 5GB disk cap]\n` is written and all
    /// subsequent appends are dropped. The write itself uses
    /// [`FileSystem::append_file_no_follow`] so a symlink planted at the spool
    /// path from inside the sandbox cannot redirect it (T18).
    pub async fn append(&self, output_file: &Path, content: &str) -> Result<(), OutputError> {
        let relative = self.relative_path_for(output_file)?;
        let write_lock = self.write_lock_for(output_file).await;
        let _writes = write_lock.lock().await;
        if self.terminal.lock().await.contains(output_file) {
            return Ok(());
        }
        let root_identity = self.check_output_root().await?;
        // Determine what to write under the cap, holding the per-path state lock
        // only across the cheap bookkeeping (not the await on the fs write).
        let to_write = {
            let mut caps = self.caps.lock().await;
            let state = caps.entry(output_file.to_path_buf()).or_default();
            if state.capped {
                // Already capped — drop further output (claude `if (capped) return`).
                None
            } else {
                // `this.#f += t.length` — UTF-16 code units, matching JS
                // `String.length`.
                state.bytes_written =
                    state.bytes_written.saturating_add(utf16_units(content));
                if state.bytes_written > MAX_TASK_OUTPUT_BYTES {
                    state.capped = true;
                    Some(format!(
                        "\n[output truncated: exceeded {MAX_TASK_OUTPUT_BYTES_DISPLAY} disk cap]\n"
                    ))
                } else {
                    Some(content.to_string())
                }
            }
        };
        if let Some(body) = to_write {
            let first = self
                .fs
                .append_file_rooted_no_follow_pinned(
                    &self.output_dir,
                    &relative,
                    &body,
                    root_identity.as_ref(),
                )
                .await;
            if let Err(error) = first {
                // TOF-05. claude-code retries a failed drain EXACTLY once, and
                // the retry does NOT re-issue the chunk: `#p()` splices the
                // buffer before the await, so by the time the catch runs the
                // original body is gone and only the marker is queued. The
                // chunk is deliberately lost — the retry exists to get the
                // MARKER on disk, not the output.
                //
                // `lostOutput` is set in the INNER catch, i.e. on this first
                // failure, and the oracle never clears it: `#w()` resets
                // `failing` and the reported-error set, `cancel()` resets the
                // queue, neither touches `#l`. Once a spool has lost output it
                // has lost it.
                {
                    let mut caps = self.caps.lock().await;
                    let state = caps.entry(output_file.to_path_buf()).or_default();
                    state.lost_output = true;
                }
                tracing::error!(
                    "Task output drain failed (will retry once): {}",
                    self.map_rooted_error(error)
                );
                self.fs
                    .append_file_rooted_no_follow_pinned(
                        &self.output_dir,
                        &relative,
                        OUTPUT_OMITTED_MARKER,
                        root_identity.as_ref(),
                    )
                    .await
                    .map_err(|e| self.map_rooted_error(e))?;
            }
        }
        Ok(())
    }

    /// Whether this spool has ever failed a write — claude-code `lostOutput`.
    ///
    /// SET-ONCE: the oracle stamps it in the drain's inner catch and never
    /// clears it, so a spool that lost a chunk keeps saying so even after later
    /// writes succeed. Its reader upstream is the `TaskOutput` footer, which
    /// swaps "Full output saved to: {path}" for a sentence saying the file may
    /// be incomplete — a surface this port does not have yet, which is why this
    /// accessor currently has no production caller.
    pub async fn lost_output(&self, output_file: &Path) -> bool {
        self.caps
            .lock()
            .await
            .get(output_file)
            .is_some_and(|state| state.lost_output)
    }

    /// Replace a terminal task payload in its already-allocated spool.
    ///
    /// This is deliberately narrower than the generic task-output API: the
    /// mobile Local App completion sink calls it only after authenticating a
    /// Host QA result (or constructing a fail-closed terminal error). The
    /// path is validated against this manager's output root, the root identity
    /// is pinned before and after the rooted atomic write, and replacement is
    /// serialized with appends so raw handler output cannot win a race with
    /// the checked terminal payload. Once a fail-closed failure override has
    /// been installed, this method refuses a late success replacement.
    pub async fn replace_terminal_result(
        &self,
        output_file: &Path,
        content: &str,
    ) -> Result<(), OutputError> {
        let relative = self.relative_path_for(output_file)?;
        let write_lock = self.write_lock_for(output_file).await;
        let _writes = write_lock.lock().await;
        let _root_identity = self.check_output_root().await?;
        if self
            .terminal_overrides
            .lock()
            .await
            .contains_key(output_file)
        {
            return Err(OutputError::Io(
                "terminal override is already authoritative".into(),
            ));
        }
        self.validate_terminal_result(output_file, content)?;
        self.fs
            .write_file_rooted_atomic(&self.output_dir, &relative, content)
            .await
            .map_err(|e| self.map_rooted_error(e))?;
        // The rooted atomic write is platform-confined; this second identity
        // check also rejects a directory swap that raced the operation before
        // the next append/read can proceed.
        self.check_output_root().await?;
        self.terminal.lock().await.insert(output_file.to_path_buf());
        let mut caps = self.caps.lock().await;
        caps.insert(
            output_file.to_path_buf(),
            CapState {
                // Same unit as `append`: UTF-16 code units.
                bytes_written: utf16_units(content),
                capped: false,
                // A terminal replacement is a fresh, complete payload.
                lost_output: false,
            },
        );
        Ok(())
    }

    pub(crate) fn validate_terminal_result(
        &self,
        output_file: &Path,
        content: &str,
    ) -> Result<(), OutputError> {
        self.relative_path_for(output_file)?;
        if content.len() as u64 > MAX_TASK_OUTPUT_BYTES {
            return Err(OutputError::Io(format!(
                "terminal output exceeds {MAX_TASK_OUTPUT_BYTES_DISPLAY} disk cap"
            )));
        }
        Ok(())
    }

    /// Make a bounded terminal payload authoritative before best-effort disk
    /// persistence. An error from this method means the in-memory projection is
    /// still authoritative but the physical spool must not be exposed as such.
    pub(crate) async fn replace_terminal_result_authoritative(
        &self,
        output_file: &Path,
        content: &str,
    ) -> Result<(), OutputError> {
        let relative = self.relative_path_for(output_file)?;
        let write_lock = self.write_lock_for(output_file).await;
        let _writes = write_lock.lock().await;
        self.validate_terminal_result(output_file, content)?;

        self.terminal_overrides.lock().await.insert(
            output_file.to_path_buf(),
            TerminalOverride {
                content: content.to_string(),
                physical_spool_authoritative: false,
            },
        );
        self.terminal.lock().await.insert(output_file.to_path_buf());

        self.check_output_root().await?;
        self.fs
            .write_file_rooted_atomic(&self.output_dir, &relative, content)
            .await
            .map_err(|error| self.map_rooted_error(error))?;
        self.check_output_root().await?;

        if let Some(terminal_override) = self.terminal_overrides.lock().await.get_mut(output_file) {
            terminal_override.physical_spool_authoritative = true;
        }
        let mut caps = self.caps.lock().await;
        caps.insert(
            output_file.to_path_buf(),
            CapState {
                // Same unit as `append`: UTF-16 code units.
                bytes_written: utf16_units(content),
                capped: false,
                // A terminal replacement is a fresh, complete payload.
                lost_output: false,
            },
        );
        Ok(())
    }

    /// Publish a terminal failure with a fail-closed read path.
    ///
    /// The in-memory override becomes authoritative before the filesystem
    /// write. If the write fails (for example, because the disk is full), all
    /// subsequent reads still return this failure payload instead of stale
    /// success bytes left in the spool. The override also makes late handler
    /// appends terminal and is intentionally absorbing for this spool.
    pub async fn replace_terminal_result_fail_closed(
        &self,
        output_file: &Path,
        content: &str,
    ) -> Result<(), OutputError> {
        self.replace_terminal_result_authoritative(output_file, content)
            .await
    }

    pub(crate) async fn physical_output_is_authoritative(&self, output_file: &Path) -> bool {
        let override_state = self
            .terminal_overrides
            .lock()
            .await
            .get(output_file)
            .map(|terminal_override| terminal_override.physical_spool_authoritative);
        match override_state {
            None => true,
            Some(false) => false,
            Some(true) => self.check_output_root().await.is_ok(),
        }
    }

    /// Test-only accessor for the backing filesystem (so M5-01 tests can
    /// seed spool content directly without going through a handler).
    #[doc(hidden)]
    pub fn fs_for_test(&self) -> Arc<dyn FileSystem> {
        self.fs.clone()
    }

    /// Test-only: pre-seed the per-path write-side byte counter, so the 5GB cap
    /// can be exercised without materializing 5GB of output.
    #[cfg(test)]
    async fn seed_bytes_for_test(&self, output_file: &Path, bytes: u64) {
        let mut caps = self.caps.lock().await;
        caps.entry(output_file.to_path_buf())
            .or_default()
            .bytes_written = bytes;
    }

    /// Read a window of the task's spool file.
    ///
    /// The per-spool lock is intentional: terminal publication records its
    /// authoritative override before attempting the filesystem rewrite, and a
    /// read must observe either that override or the pre-terminal file, never
    /// stale physical bytes after the registry has committed a terminal task.
    pub async fn read(
        &self,
        output_file: &Path,
        opts: OutputOptions,
    ) -> Result<TaskOutput, OutputError> {
        let relative = self.relative_path_for(output_file)?;
        let write_lock = self.write_lock_for(output_file).await;
        let _writes = write_lock.lock().await;
        if let Some(terminal_override) = self
            .terminal_overrides
            .lock()
            .await
            .get(output_file)
            .cloned()
        {
            let physical_spool_authoritative = terminal_override.physical_spool_authoritative
                && self.check_output_root().await.is_ok();
            let fc =
                platform_api::apply_line_window(terminal_override.content, opts.offset, opts.limit);
            return Ok(TaskOutput {
                content: cap_full_read(fc.content, &opts),
                total_lines: fc.total_lines,
                truncated: fc.truncated,
                physical_spool_authoritative,
            });
        }
        let root_identity = self.check_output_root().await?;
        let fc = self
            .fs
            .read_file_rooted_no_follow_window_pinned(
                &self.output_dir,
                &relative,
                opts.offset,
                opts.limit,
                root_identity.as_ref(),
            )
            .await
            .map_err(|e| OutputError::Io(e.to_string()))?;
        Ok(TaskOutput {
            content: cap_full_read(fc.content, &opts),
            total_lines: fc.total_lines,
            truncated: fc.truncated,
            physical_spool_authoritative: true,
        })
    }
}

/// Apply [`apply_read_tail_cap`] only to a FULL read.
///
/// A windowed read is the caller asking for a specific slice; the oracle's
/// byte cap belongs to `getTaskOutput`, which takes no window. `total_lines`
/// deliberately still counts the whole file — the header says what was dropped,
/// and the line total is what the caller pages against.
fn cap_full_read(content: String, opts: &OutputOptions) -> String {
    if opts.offset.is_some() || opts.limit.is_some() {
        return content;
    }
    apply_read_tail_cap(content)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use platform_api::filesystem::{FileContent, FileEvent, FlockGuard, FsError};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Mutex;

    /// In-memory [`FileSystem`] with REAL exclusive-create semantics: a
    /// `create_new_file` for a path that already has a key fails with
    /// [`FsError::AlreadyExists`] (mirroring `O_EXCL`), and never overwrites the
    /// stored bytes. Also counts `create_new_file` calls so a test can assert
    /// the registry does not re-allocate.
    struct ExclusiveFs {
        files: Mutex<HashMap<String, String>>,
        creates: AtomicUsize,
        /// TOF-05: fail the next N appends, so the retry path is reachable.
        fail_appends: AtomicUsize,
        /// Every body `append_file` was ASKED to write, failed ones included —
        /// the retry writes a different body from the first attempt, and that
        /// difference is the whole point.
        append_bodies: Mutex<Vec<String>>,
    }
    impl ExclusiveFs {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                files: Mutex::new(HashMap::new()),
                creates: AtomicUsize::new(0),
                fail_appends: AtomicUsize::new(0),
                append_bodies: Mutex::new(Vec::new()),
            })
        }
        fn create_count(&self) -> usize {
            self.creates.load(Ordering::SeqCst)
        }
        fn fail_next_appends(&self, n: usize) {
            self.fail_appends.store(n, Ordering::SeqCst);
        }
        async fn append_bodies(&self) -> Vec<String> {
            self.append_bodies.lock().await.clone()
        }
    }
    #[async_trait]
    impl FileSystem for ExclusiveFs {
        async fn root_identity_no_follow(
            &self,
            root: &Path,
        ) -> Result<Option<platform_api::rooted_fs::RootIdentity>, FsError> {
            if root.exists() {
                platform_api::rooted_fs::root_identity(root).map(Some)
            } else {
                Ok(None)
            }
        }

        async fn read_file(
            &self,
            path: &str,
            _offset: Option<u64>,
            _limit: Option<u64>,
        ) -> Result<FileContent, FsError> {
            let map = self.files.lock().await;
            let content = map.get(path).cloned().unwrap_or_default();
            let total_lines = content.lines().count() as u64;
            Ok(FileContent {
                content,
                truncated: false,
                total_lines,
            })
        }
        async fn write_file(&self, path: &str, body: &str) -> Result<(), FsError> {
            self.files
                .lock()
                .await
                .insert(path.to_string(), body.to_string());
            Ok(())
        }
        async fn create_new_file(&self, path: &str) -> Result<(), FsError> {
            self.creates.fetch_add(1, Ordering::SeqCst);
            let mut map = self.files.lock().await;
            if map.contains_key(path) {
                // O_EXCL collision — refuse, and DO NOT touch the stored bytes.
                return Err(FsError::AlreadyExists(path.to_string()));
            }
            map.insert(path.to_string(), String::new());
            Ok(())
        }
        fn is_within_workspace(&self, _: &str) -> bool {
            true
        }
        async fn watch(
            &self,
            _: &str,
        ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = FileEvent> + Send>>, FsError>
        {
            Err(FsError::Io("not supported".into()))
        }
        async fn append_file(&self, path: &str, body: &str) -> Result<(), FsError> {
            self.append_bodies.lock().await.push(body.to_string());
            if self
                .fail_appends
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                    n.checked_sub(1)
                })
                .is_ok()
            {
                return Err(FsError::Io("no space left on device".into()));
            }
            self.files
                .lock()
                .await
                .entry(path.to_string())
                .or_default()
                .push_str(body);
            Ok(())
        }
        async fn truncate(&self, _: &str, _: u64) -> Result<(), FsError> {
            Ok(())
        }
        async fn file_mtime(&self, _: &str) -> Result<std::time::SystemTime, FsError> {
            Ok(std::time::SystemTime::UNIX_EPOCH)
        }
        async fn file_size(&self, path: &str) -> Result<u64, FsError> {
            let map = self.files.lock().await;
            Ok(map.get(path).map_or(0, |s| s.len() as u64))
        }
        async fn delete_file(&self, path: &str) -> Result<(), FsError> {
            self.files.lock().await.remove(path);
            Ok(())
        }
        async fn symlink(&self, _: &str, _: &str) -> Result<(), FsError> {
            Ok(())
        }
        async fn flock_exclusive(&self, _: &str) -> Result<Box<dyn FlockGuard>, FsError> {
            Err(FsError::Io("not supported".into()))
        }
        async fn fsync(&self, _: &str) -> Result<(), FsError> {
            Ok(())
        }
    }

    fn manager() -> (Arc<ExclusiveFs>, TaskOutputManager) {
        let fs = ExclusiveFs::new();
        let mgr = TaskOutputManager::new(PathBuf::from("/spool"), fs.clone());
        (fs, mgr)
    }

    #[tokio::test]
    async fn allocate_creates_an_empty_spool_file() {
        let (fs, mgr) = manager();
        let path = mgr.allocate("bdeadbeef").await.expect("first allocate");
        assert_eq!(path, PathBuf::from("/spool/bdeadbeef.output"));
        // The file exists and is empty.
        assert!(fs
            .files
            .lock()
            .await
            .contains_key("/spool/bdeadbeef.output"));
    }

    #[tokio::test]
    async fn second_allocate_of_same_id_errors_not_truncates() {
        // T4: a second allocate of the SAME id must surface AlreadyExists, not
        // silently truncate output a worker already appended.
        let (fs, mgr) = manager();
        let path = mgr.allocate("babc123").await.unwrap();

        // A worker appends output AFTER the first allocate.
        fs.append_file_no_follow(path.to_str().unwrap(), "important output\n")
            .await
            .unwrap();

        // The second allocate must error rather than clobber.
        let err = mgr
            .allocate("babc123")
            .await
            .expect_err("a second allocate of the same id must error");
        assert!(
            matches!(err, OutputError::AlreadyExists(_)),
            "second allocate returns AlreadyExists; got {err:?}"
        );

        // The worker's output survived — the refused allocate touched no bytes.
        let read = mgr.read(&path, OutputOptions::default()).await.unwrap();
        assert!(
            read.content.contains("important output"),
            "the existing output was NOT truncated by the refused allocate; got {:?}",
            read.content
        );
    }

    #[tokio::test]
    async fn path_for_does_not_create_or_count_a_spool_file() {
        // `path_for` (the registry's consume-the-handler's-path seam) only
        // reconstructs the deterministic path — it allocates nothing, so a
        // worker's already-allocated spool is never re-created.
        let (fs, mgr) = manager();
        let path = mgr.path_for("bxyz").unwrap();
        assert_eq!(path, PathBuf::from("/spool/bxyz.output"));
        assert_eq!(fs.create_count(), 0, "path_for must not create a file");
        assert!(
            !fs.files.lock().await.contains_key("/spool/bxyz.output"),
            "path_for must not materialize the spool"
        );
    }

    #[tokio::test]
    async fn append_writes_through_and_under_the_cap_is_unmarked() {
        // Below the 5GB cap, `append` writes the chunk verbatim — no marker.
        let (_fs, mgr) = manager();
        let path = mgr.allocate("bappend01").await.unwrap();
        mgr.append(&path, "hello ").await.unwrap();
        mgr.append(&path, "world\n").await.unwrap();
        let read = mgr.read(&path, OutputOptions::default()).await.unwrap();
        assert_eq!(read.content, "hello world\n");
        assert!(
            !read.content.contains("disk cap"),
            "no truncation marker under the cap; got {:?}",
            read.content
        );
    }

    #[tokio::test]
    async fn terminal_replacement_overwrites_raw_and_blocks_late_append() {
        let (_fs, mgr) = manager();
        let path = mgr.allocate("bterminal1").await.unwrap();
        mgr.append(&path, "{\"ok\":true,\"raw\":true}\n")
            .await
            .unwrap();

        mgr.replace_terminal_result(&path, "{\"ok\":true,\"verified\":true}\n")
            .await
            .unwrap();
        // A handler chunk racing after terminal publication must not make the
        // unverified payload externally visible again.
        mgr.append(&path, "forged late chunk\n").await.unwrap();

        let read = mgr.read(&path, OutputOptions::default()).await.unwrap();
        assert_eq!(read.content, "{\"ok\":true,\"verified\":true}\n");
    }

    #[tokio::test]
    async fn fail_closed_terminal_override_absorbs_a_late_success_replacement() {
        let (_fs, mgr) = manager();
        let path = mgr.allocate("bterminal2").await.unwrap();

        mgr.replace_terminal_result_fail_closed(&path, "{\"ok\":false}\n")
            .await
            .unwrap();
        let error = mgr
            .replace_terminal_result(&path, "{\"ok\":true}\n")
            .await
            .expect_err("a late success must not replace an authoritative failure");
        assert!(error.to_string().contains("already authoritative"));
        assert_eq!(
            mgr.read(&path, OutputOptions::default())
                .await
                .unwrap()
                .content,
            "{\"ok\":false}\n"
        );
    }

    #[tokio::test]
    async fn different_spools_progress_during_terminal_replacement() {
        let (_fs, mgr) = manager();
        let left = mgr.allocate("bleft0001").await.unwrap();
        let right = mgr.allocate("bright001").await.unwrap();
        let (left_result, right_result) = tokio::join!(
            mgr.replace_terminal_result(&left, "verified-left\n"),
            mgr.append(&right, "independent-right\n"),
        );
        left_result.unwrap();
        right_result.unwrap();
        assert_eq!(
            mgr.read(&left, OutputOptions::default())
                .await
                .unwrap()
                .content,
            "verified-left\n"
        );
        assert_eq!(
            mgr.read(&right, OutputOptions::default())
                .await
                .unwrap()
                .content,
            "independent-right\n"
        );
    }

    /// TOF-05: a failed write retries EXACTLY once, and the retry does NOT
    /// re-issue the chunk — the oracle splices its buffer before awaiting, so
    /// the original body is already gone by the time the catch runs. The retry
    /// exists to get the MARKER on disk, not the output. Getting this backwards
    /// (retrying the same body, or writing the marker only after a second
    /// failure) is the natural reading and it is wrong in both directions.
    #[tokio::test]
    async fn a_failed_write_retries_once_with_the_marker_not_the_chunk() {
        let (fs, mgr) = manager();
        let path = mgr.allocate("bfail0001").await.unwrap();
        fs.fail_next_appends(1);

        mgr.append(&path, "the chunk that is lost\n").await.unwrap();

        let attempts = fs.append_bodies().await;
        assert_eq!(attempts.len(), 2, "one failure, one retry: {attempts:?}");
        assert_eq!(attempts[0], "the chunk that is lost\n", "the chunk is tried first");
        assert_eq!(
            attempts[1], "\n[output omitted: it could not be written to disk]\n",
            "the RETRY carries the marker, not the chunk"
        );

        // What actually landed is the marker alone — the chunk is gone.
        let read = mgr.read(&path, OutputOptions::default()).await.unwrap();
        assert_eq!(read.content, "\n[output omitted: it could not be written to disk]\n");
        assert!(mgr.lost_output(&path).await, "the spool is known-incomplete");
    }

    /// `lostOutput` is SET-ONCE: the oracle stamps it in the drain's inner catch
    /// and never clears it — `#w()` resets `failing` and the reported set,
    /// `cancel()` resets the queue, neither touches it. A later good write does
    /// not make the spool complete again.
    #[tokio::test]
    async fn lost_output_survives_a_later_successful_write() {
        let (fs, mgr) = manager();
        let path = mgr.allocate("bfail0002").await.unwrap();
        fs.fail_next_appends(1);
        mgr.append(&path, "lost\n").await.unwrap();
        assert!(mgr.lost_output(&path).await);

        mgr.append(&path, "this one lands\n").await.unwrap();
        assert!(
            mgr.lost_output(&path).await,
            "a good write does not un-lose earlier output"
        );
        let read = mgr.read(&path, OutputOptions::default()).await.unwrap();
        assert!(read.content.ends_with("this one lands\n"), "got: {:?}", read.content);
    }

    /// A spool that never failed reports nothing lost — the flag must not be a
    /// constant `true` hiding behind two passing failure tests.
    #[tokio::test]
    async fn a_healthy_spool_reports_no_lost_output() {
        let (_fs, mgr) = manager();
        let path = mgr.allocate("bfail0003").await.unwrap();
        mgr.append(&path, "all good\n").await.unwrap();
        assert!(!mgr.lost_output(&path).await);
    }

    /// When the RETRY fails too the error surfaces to the caller — the oracle
    /// only swallows the first failure.
    #[tokio::test]
    async fn a_second_failure_is_reported_to_the_caller() {
        let (fs, mgr) = manager();
        let path = mgr.allocate("bfail0004").await.unwrap();
        fs.fail_next_appends(2);
        let err = mgr
            .append(&path, "gone\n")
            .await
            .expect_err("both attempts failed");
        assert!(matches!(err, OutputError::Io(_)), "got: {err:?}");
        assert!(mgr.lost_output(&path).await);
    }

    /// TOF-04: a full read never returns more than the last 8 MiB, and says how
    /// much it dropped. Without this a 5GB spool (which the WRITE cap happily
    /// allows) was materialised whole on its way to a caller that truncates to
    /// tens of thousands of characters.
    #[tokio::test]
    async fn a_full_read_keeps_the_last_8mb_and_announces_the_rest() {
        let (_fs, mgr) = manager();
        let path = mgr.allocate("btail0001").await.unwrap();
        // 8 MiB + 2 KiB, with a marker at each end.
        let over = 2 * 1024;
        let filler = "A".repeat(MAX_TASK_OUTPUT_READ_BYTES as usize + over - "HEAD".len() - "TAIL".len());
        mgr.append(&path, &format!("HEAD{filler}TAIL")).await.unwrap();

        let read = mgr.read(&path, OutputOptions::default()).await.unwrap();
        assert!(
            read.content.starts_with("[2KB of earlier output omitted]\n"),
            "got: {:?}",
            &read.content[..60.min(read.content.len())]
        );
        assert!(!read.content.contains("HEAD"), "the head must be dropped");
        assert!(read.content.ends_with("TAIL"), "the tail must survive");
        assert_eq!(
            read.content.len(),
            "[2KB of earlier output omitted]\n".len() + MAX_TASK_OUTPUT_READ_BYTES as usize,
            "exactly the cap survives, plus the header"
        );
    }

    /// Under the cap the content is returned verbatim — no header.
    #[tokio::test]
    async fn a_full_read_under_the_cap_is_verbatim() {
        let (_fs, mgr) = manager();
        let path = mgr.allocate("btail0002").await.unwrap();
        mgr.append(&path, "small output\n").await.unwrap();
        let read = mgr.read(&path, OutputOptions::default()).await.unwrap();
        assert_eq!(read.content, "small output\n");
    }

    /// A WINDOWED read is the caller asking for a slice; the oracle's byte cap
    /// belongs to `getTaskOutput`, which takes no window, so the header must not
    /// appear there.
    #[tokio::test]
    async fn a_windowed_read_is_not_tail_capped() {
        let (_fs, mgr) = manager();
        let path = mgr.allocate("btail0003").await.unwrap();
        let line = "B".repeat(1024);
        let body: String = std::iter::repeat(line.as_str())
            .take((MAX_TASK_OUTPUT_READ_BYTES as usize / 1025) + 64)
            .collect::<Vec<_>>()
            .join("\n");
        mgr.append(&path, &body).await.unwrap();

        let read = mgr
            .read(
                &path,
                OutputOptions {
                    offset: Some(0),
                    limit: Some(2),
                },
            )
            .await
            .unwrap();
        assert!(
            !read.content.contains("of earlier output omitted"),
            "got: {:?}",
            &read.content[..60.min(read.content.len())]
        );
    }

    /// The cap counts UTF-16 code units (JS `String.length`), not UTF-8 bytes.
    /// A CJK character is 3 bytes but 1 unit, so a byte-counting port trips the
    /// 5GB cap almost three times early on a non-ASCII spool.
    #[tokio::test]
    async fn the_cap_counts_utf16_code_units_not_bytes() {
        let (_fs, mgr) = manager();
        let path = mgr.allocate("bu16000001").await.unwrap();
        // Two units short of the cap.
        mgr.seed_bytes_for_test(&path, MAX_TASK_OUTPUT_BYTES - 2)
            .await;

        // Two CJK characters = 6 UTF-8 bytes but only 2 units, so this lands
        // exactly ON the cap and is written verbatim.
        mgr.append(&path, "\u{4f60}\u{597d}").await.unwrap();
        let body = mgr
            .read(&path, OutputOptions::default())
            .await
            .unwrap()
            .content;
        assert!(
            body.contains('\u{4f60}'),
            "at the cap the chunk is written, got: {body:?}"
        );
        assert!(
            !body.contains("output truncated"),
            "a byte count would have capped here, got: {body:?}"
        );

        // One more unit crosses it.
        mgr.append(&path, "x").await.unwrap();
        let body = mgr
            .read(&path, OutputOptions::default())
            .await
            .unwrap()
            .content;
        assert!(body.contains("output truncated"), "got: {body:?}");
    }

    #[tokio::test]
    async fn append_caps_at_5gb_and_writes_marker_then_drops() {
        // T17: claude-code `DiskTaskOutput.append` caps a single spool at
        // MAX_TASK_OUTPUT_BYTES (5GB). The byte counter uses chunk length, so
        // we exercise the boundary with a pre-seeded counter instead of
        // materializing 5GB. The first chunk that crosses the cap writes the
        // EXACT marker `\n[output truncated: exceeded 5GB disk cap]\n`;
        // subsequent appends are dropped entirely.
        let (_fs, mgr) = manager();
        let path = mgr.allocate("bcap00001").await.unwrap();

        // Seed the counter one byte below the cap, then append two bytes — the
        // running total crosses MAX_TASK_OUTPUT_BYTES in a single append.
        mgr.seed_bytes_for_test(&path, MAX_TASK_OUTPUT_BYTES - 1)
            .await;
        mgr.append(&path, "ab").await.unwrap();

        // A further write must be dropped (the spool is now capped).
        mgr.append(&path, "this must be dropped\n").await.unwrap();

        let read = mgr.read(&path, OutputOptions::default()).await.unwrap();
        assert_eq!(
            read.content, "\n[output truncated: exceeded 5GB disk cap]\n",
            "the crossing append is REPLACED by the exact marker (the raw \"ab\" \
             is not written), and post-cap appends drop"
        );
        assert_eq!(
            MAX_TASK_OUTPUT_BYTES,
            5 * 1024 * 1024 * 1024,
            "cap constant is 5GB"
        );
        assert_eq!(MAX_TASK_OUTPUT_BYTES_DISPLAY, "5GB");
    }

    /// TOF-08: the recovery clause belongs to exactly ONE refusal. Every other
    /// reason ends at the path — telling a user whose output FILE changed
    /// identity to restart with a fresh temp directory addresses nothing.
    #[cfg(unix)]
    #[tokio::test]
    async fn only_the_moved_directory_refusal_carries_recovery_guidance() {
        let dir = tempfile::tempdir().unwrap();
        let manager = TaskOutputManager::new(dir.path().join("tasks"), ExclusiveFs::new());
        let plain = manager.swap_refused("not a regular nlink-1 file");
        let OutputError::SwapRefused(message) = plain else {
            panic!("expected SwapRefused");
        };
        assert!(
            message.starts_with("task output swap refused (not a regular nlink-1 file): "),
            "got: {message}"
        );
        assert!(!message.contains("To recover"), "got: {message}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn output_root_swap_is_refused_with_recovery_guidance() {
        let parent = tempfile::tempdir().unwrap();
        let output_dir = parent.path().join("tasks");
        std::fs::create_dir(&output_dir).unwrap();
        let victim = tempfile::tempdir().unwrap();
        let fs = ExclusiveFs::new();
        let manager = TaskOutputManager::new(output_dir.clone(), fs.clone());

        // The first operation pins the real task-output directory identity.
        let terminal_path = manager.allocate("bpin0001").await.unwrap();
        manager
            .replace_terminal_result_fail_closed(&terminal_path, "authoritative\n")
            .await
            .unwrap();
        std::fs::rename(&output_dir, parent.path().join("tasks-moved")).unwrap();
        std::os::unix::fs::symlink(victim.path(), &output_dir).unwrap();

        let error = manager
            .allocate("bpin0002")
            .await
            .expect_err("a moved/symlinked task directory must fail closed");
        let OutputError::SwapRefused(message) = error else {
            panic!("expected SwapRefused, got {error:?}");
        };
        // The one refusal that carries a recovery clause — and it NAMES the
        // env var, so the reader has something to set.
        assert!(
            message.starts_with("task output swap refused (tasks dir moved or linked): "),
            "got: {message}"
        );
        assert!(
            message.contains("restart with LINGXI_TMPDIR set to a fresh directory"),
            "got: {message}"
        );
        assert!(message.contains("remove that entry itself"));
        assert!(!victim.path().join("bpin0002.output").exists());
        assert!(!fs
            .files
            .lock()
            .await
            .contains_key(&output_dir.join("bpin0002.output").display().to_string()));

        let terminal = manager
            .read(&terminal_path, OutputOptions::default())
            .await
            .expect("the authoritative in-memory terminal result remains readable");
        assert_eq!(terminal.content, "authoritative\n");
        assert!(
            !terminal.physical_spool_authoritative,
            "a replaced output root makes the previously written path stale"
        );
        assert!(
            !manager
                .physical_output_is_authoritative(&terminal_path)
                .await
        );
    }
}
