//! Resume-time session enumeration + load + chain validation + interactive picker.
//!
//! 1:1 port of `claude-code/src/commands/resume/` + `sessionStorage.ts::loadSameRepoMessageLogs`,
//! with the Ink TUI replaced by a stdio line-based picker (OQ-6 fallback).
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-08-resume.md` Task 0 for byte-locks.
//!
//! TRANSCRIPT TOLERANCE: [`load_session`] no longer enforces a strict
//! file-order linear chain. It routes the file tolerantly
//! ([`crate::jsonl::reader::JsonlReader::read_routed`]) and reconstructs the
//! main thread with a branch-aware DAG walk ([`build_conversation_chain`]),
//! faithful to `claude-code`'s `loadMessagesFromJsonlPath`
//! (`conversationRecovery.ts:416`). This lets the loader ingest a REAL
//! `claude-code` transcript (leading `summary`, interleaved
//! `attachment`/`system`, forked roots, sidechain branches).
//!
//! PARALLEL-TOOL-CALL RECOVERY — `recoverOrphanedParallelToolResults`
//! (`sessionStorage.ts:2096`): the post-walk DAG recovery pass that re-attaches
//! sibling assistant blocks + orphaned `tool_result`s produced by PARALLEL tool
//! calls (N `tool_use`s → N one-block assistant messages sharing `message.id`)
//! now runs as an additive post-pass
//! ([`recover_orphaned_parallel_tool_results`]) at the tail of
//! [`build_conversation_chain`]. The single-parent walk still keeps one branch;
//! the post-pass then splices each group's off-chain siblings + `tool_results` in
//! right after their on-chain anchor, never reordering the main chain.

use crate::jsonl::path::{project_dir_name, session_path};
use crate::jsonl::reader::{JsonlReader, LoadedTranscript};
use crate::jsonl::schema::JsonlMessage;
use serde_json::Value;
use crate::jsonl::title::{extract_title, truncate_title};
use std::collections::HashSet;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::SystemTime;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use traits::FileSystem;
use uuid::Uuid;

/// Metadata for one resumable session row (uuid + title + mtime + created + line count).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMetadata {
    /// The session UUID parsed from the filename stem.
    pub uuid: Uuid,
    /// The session title shown in the resume picker. Resolved with claude-code's
    /// precedence (custom-title > ai-title > summary > first-user-message; see
    /// [`collect_dir`]) and normalized to [`crate::jsonl::title::TITLE_MAX_CHARS`]
    /// chars + ellipsis; the display surfaces re-truncate by terminal width.
    pub title: String,
    /// File mtime (UTC `SystemTime`).
    pub modified: SystemTime,
    /// File birthtime / creation time (UTC `SystemTime`) — the parity analog of
    /// claude-code's `st.birthtime` (`sessionStorage.ts:4559`), used as the
    /// equal-`modified` tie-break. Captured from [`std::fs::Metadata::created`]
    /// at load; on platforms where `created()` is unavailable (it returns an
    /// `Err`) we fall back to [`Self::modified`], so the field is always
    /// populated and the tie-break degrades to a stable no-op rather than
    /// panicking.
    pub created: SystemTime,
    /// Number of JSONL lines in the file.
    pub message_count: usize,
    /// Absolute path to the `.jsonl` file (kept so callers can re-load without re-resolving).
    pub path: PathBuf,
}

impl Ord for SessionMetadata {
    fn cmp(&self, other: &Self) -> Ordering {
        // Newest-first (mtime desc), tie-break by `created` (birthtime) desc.
        // 1:1 with claude-code `sortLogs` (`types/logs.ts:319-330`): primary
        // `modified` DESC, then `created` DESC on equal `modified`.
        other
            .modified
            .cmp(&self.modified)
            .then_with(|| other.created.cmp(&self.created))
    }
}

impl PartialOrd for SessionMetadata {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// All errors raised by the resume layer.
#[derive(Debug, Error)]
pub enum LoaderError {
    /// The given session id / arg did not resolve to a `.jsonl` file under the cwd's project dir.
    #[error("Session {arg} was not found.")]
    SessionNotFound {
        /// The arg (UUID string) that was looked up.
        arg: String,
    },
    /// The `parentUuid` chain is broken at the named message.
    #[error("Session {arg} corrupted: parentUuid chain broken at message {at_uuid}.")]
    ChainBroken {
        /// The session arg.
        arg: String,
        /// UUID of the offending message.
        at_uuid: Uuid,
    },
    /// Two or more messages in the file claim different `sessionId` values.
    #[error("Session {arg} corrupted: sessionId mismatch (expected {expected}, got {got}).")]
    SessionIdMismatch {
        /// The session arg.
        arg: String,
        /// The session id expected (filename-derived).
        expected: Uuid,
        /// The session id observed in the offending row.
        got: Uuid,
    },
    /// The interactive picker received 3 invalid inputs in a row.
    #[error("Invalid selection (3 attempts). Aborting.")]
    InvalidSelection,
    /// I/O error during dir listing / file open / file read.
    #[error("Session {arg} I/O error: {source}")]
    Io {
        /// The path / arg the I/O was attempted against.
        arg: String,
        /// Wrapped I/O failure.
        #[source]
        source: std::io::Error,
    },
    /// The cwd's project dir has no `.jsonl` files at all.
    #[error("No conversations found to resume.")]
    EmptyDirectory,
    /// A `.jsonl` file failed to deserialize (delegates to the reader's error).
    #[error("Session {arg} parse error: {source}")]
    Parse {
        /// The arg / path.
        arg: String,
        /// Wrapped parse failure.
        #[source]
        source: serde_json::Error,
    },
}

/// Resolve `<claude_home>/projects/<sanitize(cwd)>[-djb2]` for a given cwd.
fn project_dir_for_cwd(claude_home: &Path, cwd: &str) -> PathBuf {
    claude_home.join("projects").join(project_dir_name(cwd))
}

/// Parse the `worktree ` lines of `git worktree list --porcelain` into absolute
/// path strings — 1:1 with claude-code `getWorktreePaths`'s porcelain parse
/// (`claude-code/src/utils/getWorktreePaths.ts:50-53`: keep lines starting with
/// `"worktree "`, strip that prefix).
///
/// Divergence from TS: TS applies `.normalize('NFC')` to each path; we do not
/// (no `unicode-normalization` dependency is permitted, and the rest of this
/// crate already sanitizes the cwd without NFC). The prefix comparison runs over
/// [`project_dir_name`]-sanitized strings, which map every non-`[a-zA-Z0-9]` byte
/// to `-`, so ASCII paths — the overwhelming common case — are unaffected.
/// `str::lines()` also strips a trailing `\r`, which is harmless (and slightly
/// more correct than TS on Windows).
fn parse_worktree_list(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter_map(|line| line.strip_prefix("worktree ").map(str::to_string))
        .collect()
}

/// Run `git worktree list --porcelain` in `cwd` and return the absolute worktree
/// paths, OR an empty vec on ANY failure (git missing, `cwd` unreadable, not a
/// repo, non-zero exit) **or** when the repo has a single worktree.
///
/// claude-code only cross-lists sibling worktrees when `worktreePaths.length > 1`
/// (`getStatOnlyLogsForWorktrees`, `sessionStorage.ts`); folding the `<= 1` gate
/// in here means an empty return is the single, unambiguous "behave exactly as
/// before" signal for the caller.
///
/// Uses [`std::process::Command`] (no new dependency; `tokio`'s `process` feature
/// is not enabled in this crate). `output()` blocks the calling task briefly,
/// which is acceptable for the one-shot, interactive `/resume` entry point.
fn git_worktree_paths(cwd: &str) -> Vec<String> {
    let output = match Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(cwd)
        .output()
    {
        Ok(out) if out.status.success() => out,
        _ => return Vec::new(),
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let paths = parse_worktree_list(&stdout);
    if paths.len() <= 1 {
        Vec::new()
    } else {
        paths
    }
}

/// claude-code's worktree dir-name match
/// (`getStatOnlyLogsForWorktrees`, `sessionStorage.ts`):
/// `dirName === prefix || dirName.startsWith(prefix + '-')`.
///
/// The `startsWith(prefix + '-')` arm catches sessions launched in a
/// SUBDIRECTORY of the worktree, whose sanitized project-dir name is
/// `<prefix>-<sanitized-subpath>`. The trailing `-` is load-bearing: it stops a
/// prefix like `-x-repo` from matching an unrelated `-x-repository`.
fn worktree_dir_matches(dir_name: &str, prefix: &str) -> bool {
    dir_name == prefix
        || dir_name
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('-'))
}

/// Deduplicate session rows by session id (the filename UUID), keeping the row
/// with the newest `modified` time — 1:1 with claude-code
/// `deduplicateLogsBySessionId` (`sessionStorage.ts:4955`), whose
/// `log.modified.getTime() > existing.modified.getTime()` replaces and keeps the
/// first-seen entry on a tie. The same session can appear under multiple
/// worktree project dirs; this collapses it to one.
fn deduplicate_by_session_id(rows: Vec<SessionMetadata>) -> Vec<SessionMetadata> {
    let mut by_id: HashMap<Uuid, SessionMetadata> = HashMap::with_capacity(rows.len());
    for row in rows {
        match by_id.get(&row.uuid) {
            // Keep the existing row unless the incoming one is STRICTLY newer.
            Some(existing) if existing.modified >= row.modified => {}
            _ => {
                by_id.insert(row.uuid, row);
            }
        }
    }
    by_id.into_values().collect()
}

/// Scan a single project dir, appending one [`SessionMetadata`] row per resumable
/// `.jsonl` file to `rows`. Applies the SESSION.1 sidechain/`teamName` hide
/// filter (first parsed line only). Each row's `title` follows claude-code's
/// display precedence — `custom-title` > `ai-title` > `summary` (keyed by the
/// chain tip's `leafUuid`) > first-user-message — composed from
/// `readLiteMetadata`'s custom-over-ai rule (`sessionStorage.ts:4771-4775`) and
/// `getLogDisplayTitle` (`utils/log.ts:30`). Returns `Ok(false)` when the dir
/// does not exist (`NotFound`) and `Ok(true)` when it was read; I/O errors carry
/// the offending path as `arg`, exactly as the original single-dir scan did.
async fn collect_dir(
    dir: &Path,
    fs: &Arc<dyn FileSystem>,
    rows: &mut Vec<SessionMetadata>,
) -> Result<bool, LoaderError> {
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(source) => {
            return Err(LoaderError::Io {
                arg: dir.display().to_string(),
                source,
            });
        }
    };

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|source| LoaderError::Io {
            arg: dir.display().to_string(),
            source,
        })?
    {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        let metadata = entry.metadata().await.map_err(|source| LoaderError::Io {
            arg: path.display().to_string(),
            source,
        })?;
        let modified = metadata.modified().map_err(|source| LoaderError::Io {
            arg: path.display().to_string(),
            source,
        })?;
        // `created()` is the parity analog of TS `st.birthtime`. Unlike
        // `modified()` it is NOT available on every platform/filesystem — it
        // returns `Err` where birthtime is unsupported — so we fall back to
        // `modified` there (the equal-mtime tie-break then degrades to a stable
        // no-op rather than failing the whole scan). No new dependency: this is
        // std-only `std::fs::Metadata::created`.
        let created = metadata.created().unwrap_or(modified);

        // Parse uuid from filename stem; silently skip non-UUID files.
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Ok(uuid) = Uuid::parse_str(stem) else {
            continue;
        };

        // Route the file TOLERANTLY ([`read_routed`]) rather than `read_all`:
        // besides the chain participants (`messages_in_order`, identical to what
        // `read_all` returned) it yields the Tier-1 metadata side-maps the picker
        // needs to surface a session's stored title — `summaries` (keyed by
        // leafUuid), `custom_titles` + `ai_titles` (keyed by sessionId). See
        // `LoadedTranscript`.
        let reader = JsonlReader::new(path.clone(), fs.clone());
        let loaded = reader.read_routed().await.map_err(|e| LoaderError::Io {
            arg: path.display().to_string(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()),
        })?;

        // SESSION.1 — claude-code HIDES sub-agent / sidechain transcripts from
        // the /resume picker. The decision is made from the FIRST line only:
        //   - `parseSessionInfoFromLite` returns null when the file's first line
        //     contains `"isSidechain":true` (listSessionsImpl.ts:88-95);
        //   - `enrichLog` returns null when the first entry `isSidechain` OR
        //     carries a truthy `teamName` (sessionStorage.ts:5055-5067);
        //   - `filterResumableSessions` drops `l.isSidechain` (resume picker).
        // Mirror that: inspect only the first chain-participant line (the first
        // parsed transcript line — we do NOT scan the whole file for the decision)
        // and skip the session when it is a sidechain message or carries a truthy
        // `teamName`. `teamName` is an outer field captured in
        // `JsonlMessage::extra`; the truthiness test matches TS
        // `if (enriched.teamName)` (an empty-string teamName is falsy).
        if let Some(first) = loaded.messages_in_order.first() {
            let has_team_name = first.extra.get("teamName").is_some_and(|v| match v {
                serde_json::Value::Null => false,
                serde_json::Value::String(s) => !s.is_empty(),
                _ => true,
            });
            if first.is_sidechain || has_team_name {
                continue;
            }

            // Gap #2 fix — SESSION.2: filter `sessionKind` daemon sessions.
            // Binary `vkm` (@ 206492423):
            //   `if(i.sessionKind==="daemon"||i.sessionKind==="daemon-worker") return C(...),null`
            // Binary log: `"$ filtered from /resume: sessionKind="` @ 113414433.
            // `sessionKind` is carried in `extra` (outer field, not a named struct field).
            let session_kind = first.extra.get("sessionKind").and_then(|v| v.as_str()).unwrap_or("");
            if session_kind == "daemon" || session_kind == "daemon-worker" {
                continue;
            }

            // Gap #3 fix — SESSION.3: filter SDK-entrypoint sessions.
            // Binary `vkm`: `k9l=new Set(["sdk-cli","sdk-ts","sdk-py"])`;
            //   `if(!a && k9l.has(n.entrypoint??"")) return C(...),null`
            // Binary log: `"# filtered from /resume: entrypoint="` @ 113414513.
            // The `!a` guard means this only applies when the session is NOT already
            // flagged (i.e., passes the daemon check above). The entrypoint field IS
            // a named struct field on JsonlMessage.
            let entrypoint = first.entrypoint.as_deref().unwrap_or("");
            if matches!(entrypoint, "sdk-cli" | "sdk-ts" | "sdk-py") {
                continue;
            }
        }

        // Gap #4 fix — SESSION.4: filter `/loop` sessions.
        // Binary `vkm`: `m=r.includes("<command-name>/loop</command-name>")` (where
        // `r` is the first line raw string), then
        //   `if(!a&&n.isLoopSession) return C(...),null`
        // Binary log: `"% filtered from /resume: /loop session"` @ 113414577.
        // Confirmed string: `"<command-name>/loop</command-name>"` @ 113388700.
        // The binary sets `isLoopSession` by scanning the FIRST LINE (the raw
        // string) for the `/loop` command-name tag — we scan the first message's
        // content text for the same tag. We do this OUTSIDE the `if let Some(first)`
        // block so we don't shadow the first-check path; the session is only reachable
        // here when it has at least one message (the block above `continue`d otherwise).
        {
            let raw_first_line = {
                // Reconstruct the first raw JSONL line from messages_in_order[0] to
                // check for the /loop tag. Because route_lines drops raw text after
                // parse, we must re-serialize the message back to JSON and scan it.
                // This is cheap (one message) and correct — the tag appears verbatim
                // in the user message content.
                loaded
                    .messages_in_order
                    .first()
                    .and_then(|m| serde_json::to_string(m).ok())
                    .unwrap_or_default()
            };
            if raw_first_line.contains("<command-name>/loop</command-name>") {
                continue;
            }
        }

        // Title precedence — 1:1 with claude-code's resolution, which composes
        // `readLiteMetadata` (custom-title field wins over ai-title field;
        // `sessionStorage.ts:4771-4775`) with `getLogDisplayTitle`
        // (`customTitle || summary || firstPrompt`; `utils/log.ts:30`). Folded:
        //   custom-title > ai-title > summary(@ tip leafUuid) > first-user-message.
        // Custom (user rename) ALWAYS wins over an AI title — `logs.ts:69`
        // "User renames (custom-title) always win over AI titles in read
        // preference". `custom_titles`/`ai_titles` are keyed by sessionId; for a
        // resume-picker row that key is the filename stem (`sid`) — equal to the
        // chain tip's sessionId for the normal, non-forked sessions the picker
        // lists. The `summary` is keyed by the chain TIP's uuid (its `leafUuid`),
        // matching TS `summaries.get(leafMessage.uuid)` (`sessionStorage.ts:3009`).
        // The first-three sources are stored verbatim (only normalized via
        // `truncate_title`); the first-message fallback runs the full
        // `extract_title` transforms. `extract_title`'s `'(session)'` empty
        // fallback still applies when none of the four yields text.
        let sid = stem;
        let title = loaded
            .custom_titles
            .get(sid)
            .or_else(|| loaded.ai_titles.get(sid))
            .or_else(|| find_tip(&loaded, sid).and_then(|tip| loaded.summaries.get(&tip.uuid)))
            .map_or_else(
                || extract_title(&loaded.messages_in_order),
                |t| truncate_title(t),
            );

        rows.push(SessionMetadata {
            uuid,
            title,
            modified,
            created,
            // `read_routed().messages_in_order` is exactly what `read_all`
            // returned (chain-participant lines, file order), so this preserves
            // the prior `message_count` semantics byte-for-byte.
            message_count: loaded.messages_in_order.len(),
            path,
        });
    }
    Ok(true)
}

/// Resolve the project dir for `cwd` and return up to `limit` most-recently-modified
/// `.jsonl` files as [`SessionMetadata`] rows, sorted by mtime desc (created/birthtime desc on tie).
///
/// Errors:
/// - [`LoaderError::EmptyDirectory`] if the project dir doesn't exist OR contains no `.jsonl`.
/// - [`LoaderError::Io`] on any other I/O failure.
///
/// Each row's `title` is resolved by [`collect_dir`] with claude-code's display
/// precedence — `custom-title` > `ai-title` > `summary` (at the chain tip's
/// `leafUuid`) > first-user-message ([`crate::jsonl::title::extract_title`]) —
/// from the **full** routed JSONL content (we open + parse every candidate, then
/// sort + truncate). This is O(N * lines) for N sessions; for the typical N ≤ 5
/// case (the picker limit) the cost is trivial.
///
/// Sub-agent / sidechain transcripts are HIDDEN (SESSION.1): a session is dropped
/// when its first parsed line is an `isSidechain` message or carries a truthy
/// `teamName` field, matching claude-code's `parseSessionInfoFromLite`
/// (listSessionsImpl.ts:88-95), `enrichLog` (sessionStorage.ts:5055-5067), and
/// `filterResumableSessions` (resume picker).
///
/// Locked against `claude-code/src/utils/sessionStorage.ts::loadSameRepoMessageLogs` — except:
/// - claude-code uses a 16-KiB head-only `enrichLogs` scan for the first user message; we
///   open + fully-parse because our `JsonlReader::read_all` is already in hand from M5-07.
///   (The sidechain/teamName decision still reads ONLY the first line, per TS.)
///
/// SESSION.5 — cross-worktree resume: like claude-code, when the cwd's repo has more
/// than one git worktree we ALSO surface sessions created in SIBLING worktrees of the
/// same repo. We run `git worktree list --porcelain` ([`git_worktree_paths`]), and for
/// each worktree path scan every projects-root subdir whose name matches the worktree's
/// sanitized [`project_dir_name`] prefix (`dirName === prefix || startsWith(prefix + '-')`,
/// per `getStatOnlyLogsForWorktrees`), then [`deduplicate_by_session_id`]. With 0/1
/// worktrees — or when git is unavailable / not a repo — we scan ONLY the exact cwd's
/// project dir, behaving byte-for-byte as before.
pub async fn list_recent_sessions(
    claude_home: &Path,
    cwd: &str,
    limit: usize,
    fs: Arc<dyn FileSystem>,
) -> Result<Vec<SessionMetadata>, LoaderError> {
    // `git_worktree_paths` already returns empty for git-error / non-repo /
    // single-worktree, so an empty vec is the "behave exactly as before" signal.
    let worktree_paths = git_worktree_paths(cwd);
    list_recent_sessions_inner(claude_home, cwd, limit, &fs, &worktree_paths).await
}

/// Worktree-path-injectable core of [`list_recent_sessions`] (so unit tests can
/// drive the multi-worktree branch without a real git repo). `worktree_paths`
/// empty ⇒ today's single-cwd-dir behavior; len > 1 ⇒ the SESSION.5 union.
async fn list_recent_sessions_inner(
    claude_home: &Path,
    cwd: &str,
    limit: usize,
    fs: &Arc<dyn FileSystem>,
    worktree_paths: &[String],
) -> Result<Vec<SessionMetadata>, LoaderError> {
    let mut rows: Vec<SessionMetadata> = Vec::new();

    if worktree_paths.len() <= 1 {
        // 0/1 worktrees (or git unavailable): scan ONLY the cwd's project dir.
        // `collect_dir` returns false on NotFound; the `rows.is_empty()` check
        // below collapses both "missing dir" and "no resumable files" into the
        // original `EmptyDirectory`, while other I/O errors propagate as `Io`.
        collect_dir(&project_dir_for_cwd(claude_home, cwd), fs, &mut rows).await?;
    } else {
        // > 1 worktrees: union every projects-root subdir whose name matches a
        // worktree's sanitized prefix (this also covers the cwd's own dir, since
        // the cwd is — or is under — one of the worktree paths), then dedupe by
        // session id. Mirrors `getStatOnlyLogsForWorktrees`.
        let projects_root = claude_home.join("projects");
        let prefixes: Vec<String> = worktree_paths
            .iter()
            .map(|wt| project_dir_name(wt))
            .collect();

        match tokio::fs::read_dir(&projects_root).await {
            Ok(mut entries) => {
                while let Some(entry) =
                    entries
                        .next_entry()
                        .await
                        .map_err(|source| LoaderError::Io {
                            arg: projects_root.display().to_string(),
                            source,
                        })?
                {
                    if !entry
                        .file_type()
                        .await
                        .map(|t| t.is_dir())
                        .unwrap_or(false)
                    {
                        continue;
                    }
                    let name = entry.file_name();
                    let Some(name) = name.to_str() else { continue };
                    if prefixes.iter().any(|p| worktree_dir_matches(name, p)) {
                        collect_dir(&entry.path(), fs, &mut rows).await?;
                    }
                }
            }
            // Projects root unreadable: fall back to the cwd's project dir, like
            // claude-code's `getStatOnlyLogsForWorktrees` catch branch.
            Err(_) => {
                collect_dir(&project_dir_for_cwd(claude_home, cwd), fs, &mut rows).await?;
            }
        }

        rows = deduplicate_by_session_id(rows);
    }

    if rows.is_empty() {
        return Err(LoaderError::EmptyDirectory);
    }

    rows.sort();
    rows.truncate(limit);
    Ok(rows)
}

/// Load a session by UUID and return the MAIN-THREAD conversation chain
/// (root → tip, in walk order) reconstructed by a tolerant, branch-aware DAG
/// walk over the transcript's `parentUuid` graph.
///
/// REAL-TRANSCRIPT TOLERANCE (replaces the old strict linear-chain check): a
/// genuine `claude-code` `.jsonl` is a DAG, not a file-ordered linked list —
/// it interleaves metadata lines, may carry a LEADING `summary` line, splices
/// `attachment`/`system` entries between turns, and can contain forked /
/// sidechain branches with their own leaves and (for forks) a different root
/// `sessionId`. The old `validate_chain` (msg[0].parent==None; strict
/// file-order parent links; all `session_id` equal) rejected all of these. We
/// now mirror `claude-code`'s `loadMessagesFromJsonlPath`
/// (`conversationRecovery.ts:416`): route the file
/// ([`JsonlReader::read_routed`]), pick the newest non-sidechain
/// user/assistant leaf as the tip, and walk `tip → root` via `parentUuid`
/// ([`build_conversation_chain`]). The leaf supplies the session id, so forked
/// sessions (whose root row keeps the SOURCE session's id) load cleanly.
///
/// Returns the main thread only; sidechain branches are ignored. The
/// `parentUuid` walk is cycle-guarded (breaks, never loops) and stops at a
/// missing parent (returns the partial chain) — it does NOT error on either.
///
/// Errors:
/// - [`LoaderError::SessionNotFound`] if the file doesn't exist.
/// - [`LoaderError::EmptyDirectory`] if the file has NO chain-participant
///   lines at all (nothing resumable) — surfaced via the same "nothing to
///   resume" channel callers already handle.
/// - [`LoaderError::Io`] on disk read failure.
///
/// The strict `ChainBroken` / `SessionIdMismatch` variants are RETAINED on
/// [`LoaderError`] (other code matches their `Display`) but are no longer
/// produced from this load path; structural anomalies are downgraded to a
/// `tracing::warn` + a best-effort partial chain.
pub async fn load_session(
    claude_home: &Path,
    cwd: &str,
    session_id: Uuid,
    fs: Arc<dyn FileSystem>,
) -> Result<Vec<JsonlMessage>, LoaderError> {
    let arg = session_id.to_string();
    let path = session_path(claude_home, cwd, &arg);
    if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
        return Err(LoaderError::SessionNotFound { arg });
    }
    let reader = JsonlReader::new(path, fs);
    let loaded = reader.read_routed().await.map_err(|e| LoaderError::Io {
        arg: arg.clone(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()),
    })?;

    let (chain, _tip_session_id) = build_conversation_chain(&loaded, &arg);
    if chain.is_empty() {
        // No chain-participant lines at all → nothing to resume. (A file that is
        // pure metadata, or whose only messages are sidechains.) Surface the
        // same "nothing to resume" signal callers already expect.
        return Err(LoaderError::EmptyDirectory);
    }
    Ok(chain)
}

/// Interactive line-based session picker (OQ-6 stdio fallback for the Ink TUI).
///
/// Renders:
/// ```text
/// Resume which session?
///   1. {title} [{modified}]
///   2. {title} [{modified}]
///   ...
/// >
/// ```
///
/// Behavior:
/// - Empty input line → `Ok(None)` (cancel).
/// - `1..=sessions.len()` (1-indexed) → `Ok(Some(uuid))`.
/// - Non-numeric, out-of-range, or `> sessions.len()` → print retry feedback,
///   try again up to **3 total attempts** (initial + 2 retries).
/// - After 3 failed attempts → `Err(LoaderError::InvalidSelection)`.
/// - EOF (zero-byte read) → `Ok(None)` (cancel).
///
/// `stdin` is any `AsyncBufRead`, `stdout` is any `AsyncWrite` — both passed
/// in so tests can supply `tokio::io::duplex` pairs (mirrors M5-05 permission
/// UX pattern).
///
/// Errors:
/// - [`LoaderError::EmptyDirectory`] if `sessions` is empty.
/// - [`LoaderError::InvalidSelection`] after 3 invalid inputs.
/// - [`LoaderError::Io`] on stdio read/write/flush failure.
pub async fn select_session_interactive<R, W>(
    sessions: &[SessionMetadata],
    stdin: &mut BufReader<R>,
    stdout: &mut W,
) -> Result<Option<Uuid>, LoaderError>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    if sessions.is_empty() {
        return Err(LoaderError::EmptyDirectory);
    }
    let limit = sessions.len();

    // Render header + rows once.
    let mut out_buf = String::from("Resume which session?\n");
    for (i, row) in sessions.iter().enumerate() {
        let modified_rfc3339 = format_rfc3339_seconds(row.modified);
        out_buf.push_str(&format!(
            "  {}. {} [{}]\n",
            i + 1,
            row.title,
            modified_rfc3339
        ));
    }
    stdout
        .write_all(out_buf.as_bytes())
        .await
        .map_err(|source| LoaderError::Io {
            arg: "stdout".into(),
            source,
        })?;

    for _attempt in 0..3 {
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
        if n == 0 {
            // EOF — treat as cancel.
            return Ok(None);
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        match trimmed.parse::<usize>() {
            Ok(n) if (1..=limit).contains(&n) => {
                return Ok(Some(sessions[n - 1].uuid));
            }
            _ => {
                let msg = format!("Please enter a number from 1 to {limit}, or empty to cancel.\n");
                stdout
                    .write_all(msg.as_bytes())
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

/// RFC 3339 with second precision and `Z` suffix — e.g. `2026-05-24T19:03:12Z`.
///
/// Shared formatter for the resume surfaces: the M5-08 stdio picker above and
/// the M7-12 iocraft Resume screen (`tui::screens::resume`) both call
/// this so the two surfaces render timestamps byte-for-byte identically. Pre-1970
/// inputs (never produced by file mtime on the platforms we target) fall back to
/// the Unix epoch literal.
#[must_use]
pub fn format_rfc3339_seconds(t: SystemTime) -> String {
    use std::time::UNIX_EPOCH;
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Cast to i64; pre-1970 timestamps are not produced by the OS for files we
    // care about (mtime). The session picker only ever sees positive offsets.
    #[allow(clippy::cast_possible_wrap)]
    let secs_i64 = secs as i64;
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs_i64, 0).map_or_else(
        || "1970-01-01T00:00:00Z".to_string(),
        |dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
    )
}

/// Parse an RFC 3339 transcript timestamp to a comparable millisecond epoch.
/// Unparsable / empty timestamps sort OLDEST (`i64::MIN`) so a metadata-poor or
/// malformed leaf can never win the newest-leaf race — mirrors TS
/// `new Date(m.timestamp).getTime()` where an invalid date yields `NaN` and the
/// `ts > tipTs` comparison is always false.
fn timestamp_millis(ts: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(ts)
        .map(|dt| dt.timestamp_millis())
        .unwrap_or(i64::MIN)
}

/// Select a transcript's chain TIP — the newest non-sidechain `user`/`assistant`
/// leaf — by the structural equivalent of `claude-code`'s leaf computation
/// (`sessionStorage.ts:3716`, original/non-pebble branch) + newest-non-sidechain
/// leaf selection, as composed by `loadMessagesFromJsonlPath`
/// (`conversationRecovery.ts:416`).
///
/// **Gap #1 fix — `last-prompt` explicit tip override:**
/// Before running the normal timestamp-based leaf selection, we check whether
/// the transcript carries an *explicit* `last-prompt` entry (written by
/// claude-code on every prompt submit with `policy:"always"`). Binary `Yle`
/// (@ 206473264):
/// ```text
/// else if(N.type==="last-prompt"){if(N.leafUuid)
///   L=N.explicit===true||L&&N.leafUuid===O, O=N.leafUuid}
/// …
/// V = L&&O&&n.has(O)&&!n.get(O)?.isSidechain
/// ```
/// where `L` = explicit flag and `O` = forced tip uuid. When `V` is true the
/// binary sets the tip directly to the `last-prompt` `leafUuid`, bypassing the
/// timestamp race. We mirror that: if `loaded.last_prompt_explicit` is `true`,
/// `loaded.last_prompt_leaf_uuid` names a real non-sidechain participant, and
/// that participant is a `user`/`assistant` message → return it immediately,
/// skipping steps (1)–(4). This guarantees the correct branch is resumed even
/// when two branches share the same newest timestamp.
///
/// Algorithm:
///  0. (NEW) Explicit `last-prompt` override check — return early when applicable.
///  1. `parent_uuids` = every `parentUuid` present among the chain participants.
///  2. `terminals` = participants whose `uuid` is NOT in `parent_uuids` (no
///     children) — these are the graph tips, including sidechain/orphan tips.
///  3. For each terminal, walk parents (cycle-guarded) to the nearest
///     `user`/`assistant` ancestor → that ancestor's uuid joins `leaf_uuids`.
///  4. `tip` = the `leaf_uuids` member that is a NON-sidechain `user`/`assistant`
///     message with the MAX timestamp (`>`, not `>=`, so the FIRST-seen leaf wins
///     an exact-timestamp tie — TS `if (ts > tipTs)`).
///
/// Returns `None` when there is no non-sidechain user/assistant leaf (an empty
/// graph, or a file whose only messages are sidechains).
#[must_use]
pub fn find_tip<'a>(loaded: &'a LoadedTranscript, arg: &str) -> Option<&'a JsonlMessage> {
    let by_uuid = &loaded.by_uuid;
    if by_uuid.is_empty() {
        return None;
    }

    // (0) Gap #1 fix: explicit last-prompt override.
    // Binary: `V = L&&O&&n.has(O)&&!n.get(O)?.isSidechain`
    // where L = explicit and O = leafUuid. Mirror: if explicit is set AND the
    // leafUuid is present as a non-sidechain user/assistant participant → force.
    if loaded.last_prompt_explicit {
        if let Some(lp_uuid) = &loaded.last_prompt_leaf_uuid {
            if let Some(lp_msg) = by_uuid.get(lp_uuid.as_str()) {
                if !lp_msg.is_sidechain
                    && (lp_msg.message_type == "user" || lp_msg.message_type == "assistant")
                {
                    tracing::debug!(
                        session = arg,
                        tip = %lp_uuid,
                        "last-prompt explicit override: forcing resume tip",
                    );
                    return Some(lp_msg);
                }
            }
        }
    }

    // (1) Every parentUuid that is actually referenced.
    let parent_uuids: HashSet<&str> = by_uuid
        .values()
        .filter_map(|m| m.parent_uuid.as_deref())
        .collect();

    // (2) Terminals = messages no other message points at.
    // (3) From each terminal, walk up to the nearest user/assistant leaf.
    let mut leaf_uuids: HashSet<String> = HashSet::new();
    for m in by_uuid.values() {
        if parent_uuids.contains(m.uuid.as_str()) {
            continue; // not a terminal
        }
        let mut seen: HashSet<&str> = HashSet::new();
        let mut current: Option<&JsonlMessage> = Some(m);
        while let Some(node) = current {
            if !seen.insert(node.uuid.as_str()) {
                // Cycle in the parentUuid graph — abandon this terminal's walk.
                tracing::warn!(
                    session = arg,
                    at = %node.uuid,
                    "cycle detected walking transcript leaves; skipping terminal",
                );
                break;
            }
            if node.message_type == "user" || node.message_type == "assistant" {
                leaf_uuids.insert(node.uuid.clone());
                break;
            }
            current = node
                .parent_uuid
                .as_deref()
                .and_then(|p| by_uuid.get(p));
        }
    }

    // (4) tip = newest non-sidechain user/assistant leaf.
    let mut tip: Option<&JsonlMessage> = None;
    let mut tip_ts: i64 = i64::MIN;
    for uuid in &leaf_uuids {
        let Some(m) = by_uuid.get(uuid) else { continue };
        if m.is_sidechain {
            continue;
        }
        if m.message_type != "user" && m.message_type != "assistant" {
            continue;
        }
        let ts = timestamp_millis(&m.timestamp);
        // `>` (not `>=`) so the FIRST-seen leaf wins an exact-timestamp tie,
        // matching TS `loadMessagesFromJsonlPath`'s `if (ts > tipTs)`.
        if tip.is_none() || ts > tip_ts {
            tip_ts = ts;
            tip = Some(m);
        }
    }
    tip
}

/// Tolerant, branch-aware reconstruction of a transcript's MAIN conversation
/// thread — the structural equivalent of `claude-code`'s leaf computation
/// (`sessionStorage.ts:3716`, original/non-pebble branch) + newest-non-sidechain
/// leaf selection + `buildConversationChain` (`sessionStorage.ts:2069`), as
/// composed by `loadMessagesFromJsonlPath` (`conversationRecovery.ts:416`).
///
/// Algorithm:
///  1.–4. Pick the chain TIP via [`find_tip`] (newest non-sidechain
///     `user`/`assistant` leaf). The tip — not the file's first row — supplies
///     the returned session id (forked sessions copy `chain[0]` from the source).
///  5. Walk `tip → root` via `parentUuid` + `by_uuid.get`, STOP on a missing
///     parent (partial chain, no error), BREAK on a cycle (no loop), then
///     reverse to root → tip order.
///
/// Returns `(main_thread, tip_session_id)`. When no non-sidechain
/// user/assistant leaf exists the chain is empty and the session id is the
/// requested `arg` (the caller maps the empty chain to "nothing to resume").
///
///  6. Run [`recover_orphaned_parallel_tool_results`]
///     (`recoverOrphanedParallelToolResults`, `sessionStorage.ts:2096`): the
///     single-parent walk keeps one branch, so PARALLEL tool calls (N
///     `tool_use`s → N one-block assistant siblings sharing `message.id`) leave
///     off-chain siblings + their `tool_result`s orphaned. The post-pass splices
///     each group's genuine orphans back in right after their on-chain anchor,
///     never reordering the main chain. A transcript with no parallel tool calls
///     is returned unchanged.
#[must_use]
pub fn build_conversation_chain(
    loaded: &LoadedTranscript,
    arg: &str,
) -> (Vec<JsonlMessage>, String) {
    let by_uuid = &loaded.by_uuid;
    if by_uuid.is_empty() {
        return (Vec::new(), arg.to_string());
    }

    // Steps (1)–(4): newest non-sidechain user/assistant leaf (the chain tip).
    let Some(tip) = find_tip(loaded, arg) else {
        // No resumable leaf — caller surfaces "nothing to resume".
        return (Vec::new(), arg.to_string());
    };
    let tip_session_id = tip.session_id.clone();

    // (5) Walk tip → root, cycle-guarded, stop on missing parent; reverse.
    let mut chain: Vec<JsonlMessage> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut current: Option<&JsonlMessage> = Some(tip);
    while let Some(node) = current {
        if !seen.insert(node.uuid.clone()) {
            tracing::warn!(
                session = arg,
                at = %node.uuid,
                "cycle detected in parentUuid chain; returning partial transcript",
            );
            break;
        }
        chain.push(node.clone());
        current = match node.parent_uuid.as_deref() {
            Some(p) => by_uuid.get(p), // None here ⇒ missing parent ⇒ loop ends
            None => None,              // reached the root
        };
    }
    chain.reverse();

    // (6) Recover sibling assistant blocks + tool_results the single-parent walk
    // orphaned (parallel-tool-call DAG). Additive post-pass; never reorders the
    // main chain. `seen` is exactly the set of on-chain uuids (the cycle guard
    // breaks BEFORE pushing, so no extra entries).
    let chain = recover_orphaned_parallel_tool_results(by_uuid, chain, &mut seen, arg);

    (chain, tip_session_id)
}

/// Read a chain-participant line's `message.id` (the Anthropic message id shared
/// by all sibling blocks of one streamed assistant turn). `None` when absent or
/// non-string — mirrors TS's `m.message.id` truthiness gate.
fn message_id(m: &JsonlMessage) -> Option<&str> {
    m.message.get("id").and_then(Value::as_str)
}

/// True when `m` is a `user` line whose inner `message.content` is an array
/// containing at least one `tool_result` block — 1:1 with the TS predicate
/// `m.type === 'user' && Array.isArray(m.message.content) &&
/// m.message.content.some(b => b.type === 'tool_result')`
/// (`sessionStorage.ts:2147-2151`).
fn carries_tool_result(m: &JsonlMessage) -> bool {
    if m.message_type != "user" {
        return false;
    }
    m.message
        .get("content")
        .and_then(Value::as_array)
        .is_some_and(|blocks| {
            blocks.iter().any(|b| {
                b.get("type").and_then(Value::as_str) == Some("tool_result")
            })
        })
}

/// Post-pass for [`build_conversation_chain`] — recover sibling assistant blocks
/// and `tool_result`s that the single-parent walk orphaned. 1:1 port of
/// `claude-code`'s `recoverOrphanedParallelToolResults`
/// (`sessionStorage.ts:2096-2206`).
///
/// Streaming emits one assistant message per `content_block_stop` — N parallel
/// `tool_use`s → N assistant messages with DISTINCT `uuid` but the SAME
/// `message.id`. Each `tool_result`'s `parentUuid` points at its OWN one-block
/// assistant (the write-time `sourceToolAssistantUUID` override), so the topology
/// is a DAG; the tip→root walk is a linked-list traversal that keeps only one
/// branch and drops the off-chain siblings + their `tool_results`. This pass
/// re-attaches them.
///
/// Conservative by construction: it only ever ADDS genuine orphans (members not
/// already in `seen`), splices each group's recovered entries immediately AFTER
/// the group's last on-chain anchor, and never moves an existing chain member —
/// so a transcript without parallel tool calls is returned byte-identical.
fn recover_orphaned_parallel_tool_results(
    by_uuid: &HashMap<String, JsonlMessage>,
    chain: Vec<JsonlMessage>,
    seen: &mut HashSet<String>,
    arg: &str,
) -> Vec<JsonlMessage> {
    // chainAssistants — on-chain `assistant` lines, in chain order.
    let chain_assistants: Vec<&JsonlMessage> = chain
        .iter()
        .filter(|m| m.message_type == "assistant")
        .collect();
    if chain_assistants.is_empty() {
        return chain;
    }

    // anchorByMsgId — last on-chain member of each sibling group (chain order →
    // later iterations overwrite, last wins). Stores the anchor's uuid.
    let mut anchor_by_msg_id: HashMap<&str, String> = HashMap::new();
    for a in &chain_assistants {
        if let Some(id) = message_id(a) {
            anchor_by_msg_id.insert(id, a.uuid.clone());
        }
    }

    // O(n) precompute over ALL messages:
    //  - siblingsByMsgId: assistant lines grouped by `message.id`.
    //  - toolResultsByAsst: `user` tool_result carriers indexed by `parentUuid`
    //    (the write-time srcUUID; --fork-session strips srcUUID but keeps it).
    let mut siblings_by_msg_id: HashMap<&str, Vec<&JsonlMessage>> = HashMap::new();
    let mut tool_results_by_asst: HashMap<&str, Vec<&JsonlMessage>> = HashMap::new();
    for m in by_uuid.values() {
        if m.message_type == "assistant" {
            if let Some(id) = message_id(m) {
                siblings_by_msg_id.entry(id).or_default().push(m);
            }
        } else if carries_tool_result(m) {
            if let Some(parent) = m.parent_uuid.as_deref() {
                tool_results_by_asst.entry(parent).or_default().push(m);
            }
        }
    }

    // For each message.id group touching the chain: collect off-chain siblings +
    // off-chain TRs for ALL members, splice right after the group's anchor.
    let mut processed_groups: HashSet<&str> = HashSet::new();
    let mut inserts: HashMap<String, Vec<JsonlMessage>> = HashMap::new();
    let mut recovered_count: usize = 0;
    for asst in &chain_assistants {
        let Some(msg_id) = message_id(asst) else {
            continue;
        };
        if !processed_groups.insert(msg_id) {
            continue; // already handled this group
        }

        // group = siblingsByMsgId.get(msgId) ?? [asst]
        let group: Vec<&JsonlMessage> = siblings_by_msg_id
            .get(msg_id)
            .cloned()
            .unwrap_or_else(|| vec![*asst]);

        let mut orphaned_siblings: Vec<&JsonlMessage> =
            group.iter().filter(|s| !seen.contains(&s.uuid)).copied().collect();
        let mut orphaned_trs: Vec<&JsonlMessage> = Vec::new();
        for member in &group {
            if let Some(trs) = tool_results_by_asst.get(member.uuid.as_str()) {
                for tr in trs {
                    if !seen.contains(&tr.uuid) {
                        orphaned_trs.push(tr);
                    }
                }
            }
        }
        if orphaned_siblings.is_empty() && orphaned_trs.is_empty() {
            continue;
        }

        // Timestamp sort keeps content-block / completion order; the sort is
        // STABLE (`sort_by`) so JSONL read order survives ties — matching TS's
        // `localeCompare` stable sort.
        orphaned_siblings.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
        orphaned_trs.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));

        // anchor = anchorByMsgId.get(msgId)!  — guaranteed present: this group is
        // anchored by `asst`, an on-chain assistant whose id we inserted above.
        let Some(anchor_uuid) = anchor_by_msg_id.get(msg_id) else {
            continue;
        };

        let mut recovered: Vec<JsonlMessage> =
            Vec::with_capacity(orphaned_siblings.len() + orphaned_trs.len());
        for s in orphaned_siblings {
            seen.insert(s.uuid.clone());
            recovered.push(s.clone());
        }
        for tr in orphaned_trs {
            seen.insert(tr.uuid.clone());
            recovered.push(tr.clone());
        }
        recovered_count += recovered.len();
        inserts.insert(anchor_uuid.clone(), recovered);
    }

    if recovered_count == 0 {
        return chain;
    }
    tracing::debug!(
        session = arg,
        recovered = recovered_count,
        "recovered orphaned parallel tool_result blocks",
    );

    // Splice: walk the chain, append each anchor's recovered entries right after
    // it so the group stays contiguous (every TR lands after its tool_use).
    let mut result: Vec<JsonlMessage> = Vec::with_capacity(chain.len() + recovered_count);
    for m in chain {
        let recovered = inserts.remove(&m.uuid);
        result.push(m);
        if let Some(recovered) = recovered {
            result.extend(recovered);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    //! SESSION.1 coverage — `list_recent_sessions` HIDES sub-agent / sidechain
    //! transcripts (first line `isSidechain` or carrying a truthy `teamName`),
    //! matching claude-code's `parseSessionInfoFromLite` / `enrichLog` /
    //! `filterResumableSessions`. Driven over a real `tempfile` fixture via the
    //! posix `FileSystem`, mirroring `session/tests/list_recent_test.rs`.

    use super::*;
    use platform_posix::fs::PosixFileSystem;
    use std::time::Duration;
    use tempfile::TempDir;

    fn make_fs(root: &Path) -> Arc<dyn FileSystem> {
        Arc::new(PosixFileSystem::new(root.to_path_buf()))
    }

    /// Build `<claude_home>/projects/<sanitize(cwd)>/` and return
    /// `(tempdir, claude_home, cwd, project_subdir)`.
    fn setup() -> (TempDir, PathBuf, String, PathBuf) {
        let temp = TempDir::new().expect("tempdir");
        let cwd = temp
            .path()
            .join("workproj")
            .to_string_lossy()
            .into_owned();
        let claude_home = temp.path().join("home");
        let project_subdir = claude_home.join("projects").join(project_dir_name(&cwd));
        std::fs::create_dir_all(&project_subdir).expect("mkdir");
        (temp, claude_home, cwd, project_subdir)
    }

    /// Write one `<uuid>.jsonl` first-user-message session (the M5-07/M5-08
    /// on-disk shape), optionally tagging it `isSidechain` and/or `teamName`,
    /// then stamp its mtime. `prompt` becomes the row's extracted title.
    fn write_session(
        dir: &Path,
        cwd: &str,
        prompt: &str,
        mtime: SystemTime,
        is_sidechain: bool,
        team_name: Option<&str>,
    ) -> Uuid {
        let uuid = Uuid::new_v4();
        let mut line = serde_json::json!({
            "type": "user",
            "uuid": uuid.to_string(),
            "parentUuid": null,
            "sessionId": uuid.to_string(),
            "timestamp": "2026-05-25T12:00:00.000Z",
            "cwd": cwd,
            "version": "0.12.0",
            "isSidechain": is_sidechain,
            "userType": "external",
            "message": {"role": "user", "content": prompt},
        });
        if let Some(team) = team_name {
            line["teamName"] = serde_json::Value::String(team.to_string());
        }
        let bytes = format!("{}\n", serde_json::to_string(&line).unwrap());
        let path = dir.join(format!("{uuid}.jsonl"));
        std::fs::write(&path, bytes).unwrap();
        filetime::set_file_mtime(&path, filetime::FileTime::from_system_time(mtime)).unwrap();
        uuid
    }

    #[tokio::test]
    async fn excludes_sidechain_and_teamname_sessions() {
        let (temp, claude_home, cwd, dir) = setup();
        let base = SystemTime::now();
        // Newer mtimes for the hidden rows ensures they would have sorted FIRST
        // if not filtered — so a passing assertion proves the filter, not luck.
        let main = write_session(&dir, &cwd, "main prompt", base, false, None);
        let _sidechain = write_session(
            &dir,
            &cwd,
            "sub-agent transcript",
            base + Duration::from_secs(1),
            true,
            None,
        );
        let _team = write_session(
            &dir,
            &cwd,
            "team chat",
            base + Duration::from_secs(2),
            false,
            Some("squad"),
        );

        let fs = make_fs(temp.path());
        let rows = list_recent_sessions(&claude_home, &cwd, 5, fs)
            .await
            .expect("list");

        assert_eq!(rows.len(), 1, "sidechain + teamName sessions are hidden");
        assert_eq!(rows[0].uuid, main);
        assert_eq!(rows[0].title, "main prompt");
    }

    #[tokio::test]
    async fn empty_teamname_string_is_not_filtered() {
        // TS `if (enriched.teamName)` is a truthiness check — an empty-string
        // `teamName` is falsy and must NOT hide an otherwise-normal session.
        let (temp, claude_home, cwd, dir) = setup();
        let keep = write_session(&dir, &cwd, "kept", SystemTime::now(), false, Some(""));

        let fs = make_fs(temp.path());
        let rows = list_recent_sessions(&claude_home, &cwd, 5, fs)
            .await
            .expect("list");

        assert_eq!(rows.len(), 1, "empty teamName is falsy → not hidden");
        assert_eq!(rows[0].uuid, keep);
    }

    #[tokio::test]
    async fn all_sidechain_dir_is_empty_directory() {
        // If every candidate is a hidden sidechain, the picker has nothing to
        // show — same surface as a project dir with no `.jsonl` files.
        let (temp, claude_home, cwd, dir) = setup();
        let _ = write_session(&dir, &cwd, "sub a", SystemTime::now(), true, None);
        let _ = write_session(&dir, &cwd, "sub b", SystemTime::now(), true, None);

        let fs = make_fs(temp.path());
        match list_recent_sessions(&claude_home, &cwd, 5, fs).await {
            Err(LoaderError::EmptyDirectory) => {}
            other => panic!("expected EmptyDirectory, got {other:?}"),
        }
    }

    // ---- SESSION.5: cross-worktree resume --------------------------------

    /// Write one normal first-user-message session at `<dir>/<uuid>.jsonl` (the
    /// dir is created if missing), stamp its mtime, and return nothing — the
    /// caller supplies the `uuid` so the same session id can be planted in two
    /// worktree dirs to exercise dedupe.
    fn write_session_id(dir: &Path, uuid: Uuid, cwd: &str, prompt: &str, mtime: SystemTime) {
        std::fs::create_dir_all(dir).unwrap();
        let line = serde_json::json!({
            "type": "user",
            "uuid": uuid.to_string(),
            "parentUuid": null,
            "sessionId": uuid.to_string(),
            "timestamp": "2026-05-25T12:00:00.000Z",
            "cwd": cwd,
            "version": "0.12.0",
            "isSidechain": false,
            "userType": "external",
            "message": {"role": "user", "content": prompt},
        });
        let bytes = format!("{}\n", serde_json::to_string(&line).unwrap());
        let path = dir.join(format!("{uuid}.jsonl"));
        std::fs::write(&path, bytes).unwrap();
        filetime::set_file_mtime(&path, filetime::FileTime::from_system_time(mtime)).unwrap();
    }

    #[test]
    fn parses_worktree_list_porcelain() {
        // The porcelain example from getWorktreePaths.ts: keep `worktree ` lines,
        // strip the prefix; ignore HEAD/branch/blank lines.
        let stdout = "worktree /Users/foo/repo\n\
                      HEAD abc123\n\
                      branch refs/heads/main\n\
                      \n\
                      worktree /Users/foo/repo-wt1\n\
                      HEAD def456\n\
                      branch refs/heads/feature\n";
        assert_eq!(
            parse_worktree_list(stdout),
            vec![
                "/Users/foo/repo".to_string(),
                "/Users/foo/repo-wt1".to_string()
            ]
        );
        // No worktree lines → empty.
        assert!(parse_worktree_list("not a porcelain output\n").is_empty());
    }

    #[test]
    fn worktree_dir_match_rule() {
        // `dirName === prefix` and `dirName.startsWith(prefix + '-')` match…
        assert!(worktree_dir_matches("-x-repo", "-x-repo")); // exact
        assert!(worktree_dir_matches("-x-repo-sub", "-x-repo")); // subdir (prefix + '-')
        // …but a bare prefix-extension (no `-` boundary) must NOT match.
        assert!(!worktree_dir_matches("-x-repository", "-x-repo"));
        assert!(!worktree_dir_matches("-y-other", "-x-repo"));
    }

    #[test]
    fn git_unavailable_returns_no_worktrees() {
        // A throwaway dir that is not a git repo → empty (git non-zero / errors
        // out / single worktree all collapse to the same "behave as before").
        let temp = TempDir::new().unwrap();
        assert!(git_worktree_paths(&temp.path().to_string_lossy()).is_empty());
    }

    #[tokio::test]
    async fn single_worktree_scans_only_cwd_dir() {
        // `list_recent_sessions_inner` with an EMPTY worktree slice must behave
        // exactly like the pre-SESSION.5 single-dir scan: only the cwd's project
        // dir is consulted, sibling dirs are ignored.
        let (temp, claude_home, cwd, dir) = setup();
        let base = SystemTime::now();
        let main = write_session(&dir, &cwd, "main", base, false, None);

        // A sibling worktree dir exists on disk but must be invisible here.
        let sibling = claude_home
            .join("projects")
            .join(project_dir_name("/other/wt"));
        let _hidden = {
            let u = Uuid::new_v4();
            write_session_id(&sibling, u, "/other/wt", "sibling", base);
            u
        };

        let fs = make_fs(temp.path());
        let rows = list_recent_sessions_inner(&claude_home, &cwd, 5, &fs, &[])
            .await
            .expect("list");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].uuid, main);
    }

    #[tokio::test]
    async fn includes_sibling_worktree_sessions_deduped() {
        let temp = TempDir::new().unwrap();
        let claude_home = temp.path().join("home");
        let projects = claude_home.join("projects");
        std::fs::create_dir_all(&projects).unwrap();

        let wt_a = "/wt/alpha";
        let wt_b = "/wt/beta";
        let prefix_a = project_dir_name(wt_a); // "-wt-alpha"
        let prefix_b = project_dir_name(wt_b); // "-wt-beta"

        let base = SystemTime::now();

        // Exact-prefix match for worktree A.
        let dir_a = projects.join(&prefix_a);
        let a_root = Uuid::new_v4();
        write_session_id(&dir_a, a_root, wt_a, "alpha-root", base);

        // Subdir of A → matched via startsWith(prefix + '-').
        let dir_a_sub = projects.join(project_dir_name("/wt/alpha/sub"));
        let a_sub = Uuid::new_v4();
        write_session_id(
            &dir_a_sub,
            a_sub,
            "/wt/alpha/sub",
            "alpha-sub",
            base + Duration::from_secs(1),
        );

        // Exact-prefix match for worktree B.
        let dir_b = projects.join(&prefix_b);
        let b_root = Uuid::new_v4();
        write_session_id(&dir_b, b_root, wt_b, "beta-root", base + Duration::from_secs(2));

        // Boundary guard: "-wt-alphax" starts with prefix_a but the next char is
        // not '-', so it must be EXCLUDED.
        let dir_boundary = projects.join(format!("{prefix_a}x"));
        let ghost_boundary = Uuid::new_v4();
        write_session_id(
            &dir_boundary,
            ghost_boundary,
            "/wt/alphax",
            "ghost-boundary",
            base + Duration::from_secs(3),
        );

        // Unrelated dir → EXCLUDED.
        let dir_other = projects.join(project_dir_name("/some/other"));
        let ghost_other = Uuid::new_v4();
        write_session_id(
            &dir_other,
            ghost_other,
            "/some/other",
            "ghost-other",
            base + Duration::from_secs(4),
        );

        // Same session id under BOTH worktree dirs, different mtimes → dedupe
        // keeps the newest ("dup-new").
        let dup = Uuid::new_v4();
        write_session_id(&dir_a, dup, wt_a, "dup-old", base);
        write_session_id(&dir_b, dup, wt_b, "dup-new", base + Duration::from_secs(10));

        let fs = make_fs(temp.path());
        let worktrees = vec![wt_a.to_string(), wt_b.to_string()];
        let rows = list_recent_sessions_inner(&claude_home, wt_a, 50, &fs, &worktrees)
            .await
            .expect("list");

        let ids: std::collections::HashSet<Uuid> = rows.iter().map(|r| r.uuid).collect();
        assert!(ids.contains(&a_root), "alpha-root included (exact prefix)");
        assert!(ids.contains(&a_sub), "alpha-sub included (prefix + '-')");
        assert!(ids.contains(&b_root), "beta-root included (sibling worktree)");
        assert!(ids.contains(&dup), "dup session present");
        assert!(
            !ids.contains(&ghost_boundary),
            "boundary dir excluded (no '-' after prefix)"
        );
        assert!(!ids.contains(&ghost_other), "unrelated dir excluded");

        // Dedupe by id: exactly one dup row, and it is the newer one.
        let dup_rows: Vec<_> = rows.iter().filter(|r| r.uuid == dup).collect();
        assert_eq!(dup_rows.len(), 1, "dedupe collapses the duplicate session id");
        assert_eq!(dup_rows[0].title, "dup-new", "dedupe keeps the newest mtime");

        // Distinct surviving sessions: a_root, a_sub, b_root, dup.
        assert_eq!(rows.len(), 4);
    }

    #[tokio::test]
    async fn multi_worktree_empty_match_is_empty_directory() {
        // > 1 worktrees but no projects-root subdir matches any prefix → the
        // picker has nothing to resume, surfaced as EmptyDirectory (same as the
        // single-dir empty case).
        let temp = TempDir::new().unwrap();
        let claude_home = temp.path().join("home");
        std::fs::create_dir_all(claude_home.join("projects")).unwrap();

        let fs = make_fs(temp.path());
        let worktrees = vec!["/wt/alpha".to_string(), "/wt/beta".to_string()];
        match list_recent_sessions_inner(&claude_home, "/wt/alpha", 5, &fs, &worktrees).await {
            Err(LoaderError::EmptyDirectory) => {}
            other => panic!("expected EmptyDirectory, got {other:?}"),
        }
    }

    // ---- SESSION.6: equal-mtime tie-break by `created` (birthtime) DESC -----

    #[test]
    fn ord_tiebreak_prefers_newer_created() {
        // Equal `modified` → the row with the NEWER `created` (birthtime) sorts
        // first, mirroring claude-code `sortLogs`'s created-DESC tie-break
        // (`types/logs.ts:327-328`).
        let same_mtime = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let older = SessionMetadata {
            uuid: Uuid::from_u128(1),
            title: "older-created".into(),
            modified: same_mtime,
            created: SystemTime::UNIX_EPOCH + Duration::from_secs(100),
            message_count: 1,
            path: PathBuf::from("z.jsonl"),
        };
        let newer = SessionMetadata {
            uuid: Uuid::from_u128(2),
            title: "newer-created".into(),
            modified: same_mtime,
            created: SystemTime::UNIX_EPOCH + Duration::from_secs(200),
            message_count: 1,
            path: PathBuf::from("a.jsonl"),
        };
        // Insert oldest-created first to prove the sort (not insertion order)
        // drives the result.
        let mut v = vec![older, newer];
        v.sort();
        assert_eq!(v[0].title, "newer-created", "newer birthtime sorts first");
        assert_eq!(v[1].title, "older-created");
    }
}
