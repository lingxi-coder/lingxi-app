//! Durable offline reply queue for background jobs.
//!
//! When a user's follow-up reply cannot be delivered to a LIVE worker — the
//! worker already exited, its attach socket is gone, or the live write failed
//! mid-flight — Claude Code's background fleet must not silently drop the
//! message (the "失败投递无法持久化" harm: a failed delivery that cannot be
//! persisted). This module is the durable fallback: a crash-safe, per-job queue
//! of pending replies that the worker DRAINS when it (re)spawns, so the message
//! is delivered on the next turn instead of being lost.
//!
//! Layout: one file per reply under `jobs/<short>/replies/<seq>-<uuid>.json`,
//! each written atomically (`*.tmp.<pid>` sibling + rename) so a torn write is
//! never observed. `seq` is a zero-padded monotonic counter derived from the
//! current queue contents so lexical filename order == enqueue order.
//!
//! Draining CLAIMS each reply by removing its file as it is read (at-most-once).
//! This is deliberate and consistent with the daemon's fail-closed philosophy
//! (a worker that crashes mid-turn is failed closed, not resumed): a claimed
//! reply is never re-delivered, so a partial turn cannot duplicate a reply's
//! side effects.
//!
//! There is no source/binary/strings reference for the daemon orchestration
//! layer, so the on-disk shape here is a grounded engineering choice, NOT
//! byte-parity with the real binary.

use crate::agents_registry;
use std::path::{Path, PathBuf};

/// A single persisted follow-up reply awaiting delivery to a background job.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct QueuedReply {
    /// Stable per-reply id (a UUID) — also the tail of the on-disk filename.
    pub id: String,
    /// Enqueue time, epoch-millis (roster/job timestamps are epoch-millis).
    #[serde(rename = "at")]
    pub at_millis: i64,
    /// The reply text to feed the worker as a follow-up turn.
    pub text: String,
}

/// `jobs/<short>/replies/` — the per-job durable reply queue directory.
#[must_use]
pub fn replies_dir(config_home: &Path, short: &str) -> PathBuf {
    agents_registry::jobs_dir(config_home)
        .join(short)
        .join("replies")
}

/// Epoch-millis clock (matches the roster/job timestamp unit).
fn now_millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Collect the pending reply filenames under `dir`, sorted lexically (== enqueue
/// order, thanks to the zero-padded `seq` prefix). Non-`.json` entries and the
/// transient `*.tmp.*` siblings are ignored.
fn pending_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "json"))
            .collect(),
        Err(_) => Vec::new(),
    };
    files.sort();
    files
}

/// Next monotonic sequence number: one past the highest `seq` prefix already on
/// disk (0 for an empty queue). Keeps lexical filename order == enqueue order
/// even across process restarts.
fn next_seq(dir: &Path) -> u64 {
    pending_files(dir)
        .iter()
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
        .filter_map(|name| name.split('-').next())
        .filter_map(|prefix| prefix.parse::<u64>().ok())
        .max()
        .map_or(0, |max| max + 1)
}

/// Persist `text` as a pending reply for job `short`, returning the stored
/// record. The file is written to a `*.tmp.<pid>` sibling then renamed into
/// place, so a concurrent [`drain_replies`] never observes a torn line. An
/// empty/whitespace-only `text` is rejected (`None`) — there is nothing to
/// deliver.
pub fn enqueue_reply(config_home: &Path, short: &str, text: &str) -> std::io::Result<QueuedReply> {
    if text.trim().is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "empty reply text",
        ));
    }
    let dir = replies_dir(config_home, short);
    std::fs::create_dir_all(&dir)?;

    let reply = QueuedReply {
        id: uuid::Uuid::new_v4().to_string(),
        at_millis: now_millis(),
        text: text.to_string(),
    };
    let seq = next_seq(&dir);
    // 12-digit zero-pad keeps lexical order == numeric order well past any
    // realistic queue depth.
    let name = format!("{seq:012}-{}.json", reply.id);
    let target = dir.join(&name);
    let tmp = dir.join(format!("{name}.tmp.{}", std::process::id()));
    let body = serde_json::to_string(&reply).map_err(std::io::Error::other)?;
    std::fs::write(&tmp, body.as_bytes())?;
    std::fs::rename(&tmp, &target)?;
    Ok(reply)
}

/// Read the pending replies for `short` WITHOUT consuming them, in enqueue
/// order. A torn/unparsable file is skipped (a half-written `state` is never
/// observed thanks to the atomic rename, but defensiveness is cheap).
#[must_use]
pub fn peek_replies(config_home: &Path, short: &str) -> Vec<QueuedReply> {
    let dir = replies_dir(config_home, short);
    pending_files(&dir)
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .filter_map(|s| serde_json::from_str::<QueuedReply>(&s).ok())
        .collect()
}

/// Number of pending replies for `short`.
#[must_use]
pub fn pending_count(config_home: &Path, short: &str) -> usize {
    pending_files(&replies_dir(config_home, short)).len()
}

/// Drain (CLAIM) every pending reply for `short`, in enqueue order, removing
/// each file as it is read. A file that removes cleanly but fails to parse is
/// dropped (it was already claimed). See the module docs for the at-most-once
/// rationale.
#[must_use]
pub fn drain_replies(config_home: &Path, short: &str) -> Vec<QueuedReply> {
    let dir = replies_dir(config_home, short);
    let mut out = Vec::new();
    for path in pending_files(&dir) {
        // Read BEFORE remove so a mid-drain crash leaves the file for the next
        // spawn; only claim (remove) once the bytes are in hand.
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        let _ = std::fs::remove_file(&path);
        if let Ok(reply) = serde_json::from_str::<QueuedReply>(&body) {
            out.push(reply);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn tmpdir() -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut p = std::env::temp_dir();
        p.push(format!("lingxi-bgreply-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn enqueue_then_drain_preserves_order() {
        let home = tmpdir();
        enqueue_reply(&home, "job1", "first").unwrap();
        enqueue_reply(&home, "job1", "second").unwrap();
        enqueue_reply(&home, "job1", "third").unwrap();
        assert_eq!(pending_count(&home, "job1"), 3);

        let drained: Vec<String> = drain_replies(&home, "job1")
            .into_iter()
            .map(|r| r.text)
            .collect();
        assert_eq!(drained, vec!["first", "second", "third"]);
        // Draining is a one-shot claim.
        assert_eq!(pending_count(&home, "job1"), 0);
        assert!(drain_replies(&home, "job1").is_empty());
    }

    #[test]
    fn order_is_preserved_across_process_restart_via_seq_prefix() {
        // next_seq is recovered from disk, so a second enqueue "session" keeps
        // appending after the first even though nothing is held in memory.
        let home = tmpdir();
        enqueue_reply(&home, "j", "a").unwrap();
        enqueue_reply(&home, "j", "b").unwrap();
        // Simulate a restart: nothing cached, enqueue continues.
        enqueue_reply(&home, "j", "c").unwrap();
        let texts: Vec<String> = peek_replies(&home, "j")
            .into_iter()
            .map(|r| r.text)
            .collect();
        assert_eq!(texts, vec!["a", "b", "c"]);
    }

    #[test]
    fn peek_does_not_consume() {
        let home = tmpdir();
        enqueue_reply(&home, "j", "keep me").unwrap();
        assert_eq!(peek_replies(&home, "j").len(), 1);
        assert_eq!(peek_replies(&home, "j").len(), 1);
        assert_eq!(pending_count(&home, "j"), 1);
    }

    #[test]
    fn empty_text_is_rejected() {
        let home = tmpdir();
        assert!(enqueue_reply(&home, "j", "   ").is_err());
        assert!(enqueue_reply(&home, "j", "").is_err());
        assert_eq!(pending_count(&home, "j"), 0);
    }

    #[test]
    fn missing_dir_drains_and_counts_as_empty() {
        let home = tmpdir();
        assert_eq!(pending_count(&home, "never"), 0);
        assert!(drain_replies(&home, "never").is_empty());
        assert!(peek_replies(&home, "never").is_empty());
    }

    #[test]
    fn tmp_siblings_are_ignored() {
        let home = tmpdir();
        enqueue_reply(&home, "j", "real").unwrap();
        // A crashed enqueue could leave a *.tmp.<pid> sibling; it must not be
        // read as a pending reply.
        let dir = replies_dir(&home, "j");
        std::fs::write(dir.join("000000000009-x.json.tmp.999"), b"garbage").unwrap();
        assert_eq!(pending_count(&home, "j"), 1);
        let drained = drain_replies(&home, "j");
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].text, "real");
    }

    #[test]
    fn queues_are_isolated_per_job() {
        let home = tmpdir();
        enqueue_reply(&home, "alpha", "for-alpha").unwrap();
        enqueue_reply(&home, "beta", "for-beta").unwrap();
        assert_eq!(pending_count(&home, "alpha"), 1);
        assert_eq!(pending_count(&home, "beta"), 1);
        let a = drain_replies(&home, "alpha");
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].text, "for-alpha");
        // Draining alpha left beta untouched.
        assert_eq!(pending_count(&home, "beta"), 1);
    }
}
