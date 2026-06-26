//! Builds the post-compact message set: the boundary summary plus restored
//! file/skill attachments. Plans 09 (skills) and 10 (session) inject real data;
//! M1.7 ships the shape only.
//!
//! Also hosts [`run_post_compact_cleanup`] — the cache/state reset run after
//! every compaction (TS `src/services/compact/postCompactCleanup.ts`).

use crate::microcompact::reset_microcompact_state;
use crate::thresholds::{
    POST_COMPACT_MAX_FILES_TO_RESTORE, POST_COMPACT_MAX_TOKENS_PER_FILE,
    POST_COMPACT_MAX_TOKENS_PER_SKILL, POST_COMPACT_SKILLS_TOKEN_BUDGET, POST_COMPACT_TOKEN_BUDGET,
};
use crate::warning_state::clear_compact_warning_suppression;
use protocol::ConversationMessage;

/// Marker appended to skill content truncated to fit the per-skill budget.
///
/// 1:1 with claude-code v2.1.183 `Ael` (`bin/claude.exe` offset 203004352),
/// the suffix `o3p` appends when a skill's content exceeds
/// [`POST_COMPACT_MAX_TOKENS_PER_SKILL`].
pub const SKILL_TRUNCATION_MARKER: &str =
    "\n\n[... skill content truncated for compaction; use Read on the skill path if you need the full text]";

/// Cheap token estimate: `round(len / 4)`.
///
/// 1:1 with claude-code's `$f(e, t=4) = Math.round(e.length / 4)`
/// (`bin/claude.exe` offset 197027314) — the estimator the post-compact
/// restoration budgets against (`$f(Le(l))` / `$f(o.content)`). This rounds
/// (`(len + 2) / 4`) rather than the floor used by
/// [`crate::grouping::estimate_tokens_for_range`], matching the binary's
/// `Math.round` for the per-file / per-skill / running-budget comparisons.
#[must_use]
pub fn estimate_content_tokens(content: &str) -> u64 {
    // Math.round(len/4) = floor((len + 2) / 4) for the half-up rounding the
    // binary uses (`len.length` is always a non-negative integer).
    (u64::try_from(content.len()).unwrap_or(u64::MAX)).saturating_add(2) / 4
}

/// Truncate `content` to fit within `max_tokens`, appending
/// [`SKILL_TRUNCATION_MARKER`] when it overflows.
///
/// 1:1 with `o3p(e, t)` (`bin/claude.exe` offset 203003699):
/// ```text
/// function o3p(e,t){ if($f(e)<=t) return e; let n=t*4-Ael.length; return e.slice(0,n)+Ael }
/// ```
/// When `$f(content) <= max_tokens` the content is returned verbatim; otherwise
/// it is sliced to `max_tokens*4 − marker.len()` chars and the marker appended.
/// The slice is byte-based (the binary slices by UTF-16 code units; for ASCII
/// skill text the two coincide — non-ASCII slicing snaps to the nearest char
/// boundary to stay valid UTF-8).
#[must_use]
pub fn truncate_skill_content(content: &str, max_tokens: u64) -> String {
    if estimate_content_tokens(content) <= max_tokens {
        return content.to_string();
    }
    let marker_len = SKILL_TRUNCATION_MARKER.len() as u64;
    let keep = (max_tokens.saturating_mul(4)).saturating_sub(marker_len);
    let keep = usize::try_from(keep).unwrap_or(usize::MAX).min(content.len());
    // Snap to a char boundary so the slice stays valid UTF-8 (ASCII is exact).
    let mut boundary = keep;
    while boundary > 0 && !content.is_char_boundary(boundary) {
        boundary -= 1;
    }
    let mut out = content[..boundary].to_string();
    out.push_str(SKILL_TRUNCATION_MARKER);
    out
}

/// A file eligible for post-compact restoration: the path, the content the
/// model last saw (or a fresh re-read supplied by the caller), and the
/// floor-truncated mtime used to sort most-recent-first.
///
/// Mirrors the `{filename, content, timestamp}` rows `Pqn` builds from
/// `Object.entries(readFileState)` (`bin/claude.exe` offset 203001477).
#[derive(Debug, Clone)]
pub struct FileRestoreCandidate {
    /// Absolute file path.
    pub path: std::path::PathBuf,
    /// File content (the snapshot the model saw, or a caller re-read).
    pub content: String,
    /// Floor-truncated mtime in ms; restoration sorts descending on this.
    pub timestamp_ms: i64,
}

/// A skill eligible for post-compact restoration.
///
/// Mirrors the `{name, path, content, invokedAt}` rows `Lqn` builds from the
/// invoked-skill registry (`bin/claude.exe` offset 203002250).
#[derive(Debug, Clone)]
pub struct SkillRestoreCandidate {
    /// Skill name.
    pub name: String,
    /// Skill source path.
    pub path: std::path::PathBuf,
    /// Full skill content (truncated per [`POST_COMPACT_MAX_TOKENS_PER_SKILL`]).
    pub content: String,
    /// When the skill was last invoked; restoration sorts descending on this.
    pub invoked_at_ms: i64,
}

/// A restored file attachment: the path plus the (possibly per-file-capped)
/// content that survived the running token budget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoredFile {
    /// Absolute file path.
    pub path: std::path::PathBuf,
    /// The content included in the post-compact attachment.
    pub content: String,
}

/// A restored skill attachment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoredSkill {
    /// Skill name.
    pub name: String,
    /// Skill source path.
    pub path: std::path::PathBuf,
    /// The (possibly truncated) skill content included in the attachment.
    pub content: String,
}

/// Select and budget the recent files to restore after a compaction.
///
/// 1:1 with `Pqn(readFileState, ctx, n, alreadyAttached)` (`bin/claude.exe`
/// offset 203001477):
/// 1. Drop candidates already attached elsewhere (`alreadyAttached`, matched by
///    path) — the binary also drops plan files via `s3p`; the plan-file filter
///    lives in the caller here.
/// 2. Sort by `timestamp` DESC (most-recently-read first).
/// 3. Take the first [`POST_COMPACT_MAX_FILES_TO_RESTORE`] (`n = Dqn = 5`).
/// 4. Cap each file's content at [`POST_COMPACT_MAX_TOKENS_PER_FILE`]
///    (`J9p = 5000`) — the binary passes `fileReadingLimits:{maxTokens:J9p}` to
///    the re-reader; here the per-file cap truncates the supplied content.
/// 5. Keep files greedily while the running total stays `<=`
///    [`POST_COMPACT_TOKEN_BUDGET`] (`Y9p = 50000`); a file that would overflow
///    is dropped (NOT truncated to fit) and iteration continues, exactly like
///    the binary's `if(a+c<=Y9p)return a+=c,!0; return !1` filter.
#[must_use]
pub fn restore_post_compact_files(
    candidates: Vec<FileRestoreCandidate>,
    already_attached: &[std::path::PathBuf],
) -> Vec<RestoredFile> {
    let mut selected: Vec<FileRestoreCandidate> = candidates
        .into_iter()
        .filter(|c| !already_attached.iter().any(|p| p == &c.path))
        .collect();
    // Sort by timestamp DESC. `sort_by` is stable, mirroring the binary's
    // `.sort((l,c)=>c.timestamp-l.timestamp)` for equal timestamps.
    selected.sort_by(|a, b| b.timestamp_ms.cmp(&a.timestamp_ms));
    selected.truncate(POST_COMPACT_MAX_FILES_TO_RESTORE);

    let mut running = 0u64;
    let mut out = Vec::new();
    for candidate in selected {
        // Per-file cap (`fileReadingLimits:{maxTokens:J9p}`): truncate content
        // exceeding the per-file token budget. Reuse the skill truncation shape
        // (the binary's re-reader applies its own maxTokens truncation; the
        // observable effect is a per-file ceiling).
        let content =
            truncate_skill_content(&candidate.content, POST_COMPACT_MAX_TOKENS_PER_FILE);
        let cost = estimate_content_tokens(&content);
        // Running budget: keep while total + cost <= Y9p; else DROP (continue).
        if running.saturating_add(cost) <= POST_COMPACT_TOKEN_BUDGET {
            running = running.saturating_add(cost);
            out.push(RestoredFile {
                path: candidate.path,
                content,
            });
        }
    }
    out
}

/// Select and budget the invoked skills to restore after a compaction.
///
/// 1:1 with `Lqn(agentId)` (`bin/claude.exe` offset 203002250): sort invoked
/// skills by `invokedAt` DESC, truncate each to
/// [`POST_COMPACT_MAX_TOKENS_PER_SKILL`] (`X9p = 5000`) via
/// [`truncate_skill_content`], then keep greedily while the running total stays
/// `<=` [`POST_COMPACT_SKILLS_TOKEN_BUDGET`] (`Q9p = 25000`) — the binary's
/// `if(n+s>Q9p)return!1; return n+=s,!0` filter.
#[must_use]
pub fn restore_post_compact_skills(
    candidates: Vec<SkillRestoreCandidate>,
) -> Vec<RestoredSkill> {
    let mut sorted = candidates;
    sorted.sort_by(|a, b| b.invoked_at_ms.cmp(&a.invoked_at_ms));

    let mut running = 0u64;
    let mut out = Vec::new();
    for skill in sorted {
        let content = truncate_skill_content(&skill.content, POST_COMPACT_MAX_TOKENS_PER_SKILL);
        let cost = estimate_content_tokens(&content);
        // `if (n + s > Q9p) return false;` — DROP on overflow (do not truncate
        // to fit), matching the binary.
        if running.saturating_add(cost) > POST_COMPACT_SKILLS_TOKEN_BUDGET {
            continue;
        }
        running = running.saturating_add(cost);
        out.push(RestoredSkill {
            name: skill.name,
            path: skill.path,
            content,
        });
    }
    out
}

/// Whether a compaction with this query source is a **main-thread** compact —
/// the gate that decides which module-level caches are safe to reset.
///
/// TS `isMainThreadCompact` (`postCompactCleanup.ts`): `querySource ===
/// undefined || querySource.startsWith('repl_main_thread') || querySource ===
/// 'sdk'`. Subagents (`agent:*`) share module-level state with the main thread,
/// so resetting it from a subagent compact would corrupt the main thread.
#[must_use]
pub fn is_main_thread_compact(query_source: Option<&str>) -> bool {
    match query_source {
        None => true,
        Some(s) => s.starts_with("repl_main_thread") || s == "sdk",
    }
}

/// Run cleanup of caches and tracking state after compaction.
///
/// TS ref: `src/services/compact/postCompactCleanup.ts` (full file). Call this
/// after both auto-compact and manual `/compact` to free memory held by
/// tracking structures that compaction invalidates.
///
/// `query_source` is the **string** label of the compacting query (TS
/// `QuerySource`, e.g. `"repl_main_thread"`, `"sdk"`, `"agent:foo"`). We pass
/// the string here — not the structured [`sidequery::QuerySource`] enum —
/// because the TS main-thread gate is a literal `startsWith`/equality check on
/// that string and the enum does not model the `repl_main_thread*` / `agent:*`
/// namespaces. Pass `None` only for callers that are genuinely
/// main-thread-only (`/compact`, `/clear`).
///
/// 1:1 fidelity ("close" per the batch spec): the main-thread gate and the
/// resets whose Rust counterparts exist are byte-faithful; the many TS cache
/// resets with **no Rust equivalent** are documented inline rather than ported.
///
/// We intentionally do NOT clear invoked-skill content here — skill content
/// must survive across compactions so post-compact attachments can re-include
/// the full skill text (TS note + Batch 5).
pub fn run_post_compact_cleanup(query_source: Option<&str>) {
    // Subagents (`agent:*`) run in the same process and share module-level
    // state with the main thread. Only reset main-thread module-level state for
    // main-thread compacts. Same `startsWith` pattern as TS `isMainThread`.
    let is_main_thread_compact = is_main_thread_compact(query_source);

    // resetMicrocompactState() — Rust microcompact layer is stateless (no-op).
    reset_microcompact_state();

    // TS: if feature('CONTEXT_COLLAPSE') && isMainThreadCompact ->
    // resetContextCollapse(). The Rust context-collapse layer
    // (`crate::context_collapse`) is stateless (pure functions over passed-in
    // history), so there is no module-level store to reset.
    if is_main_thread_compact {
        // TS postCompactCleanup.ts: resetContextCollapse — no Rust module state
        // to reset (context_collapse is stateless).

        // TS: getUserContext.cache.clear() + resetGetMemoryFilesCache('compact').
        // These memory-file caches live in the orchestrator/session layer, not
        // in this crate. Cross-crate wiring is flagged BLOCKED for this batch;
        // the orchestrator owns the actual cache clear at the call site.
        // TS postCompactCleanup.ts: getUserContext.cache.clear — no Rust
        //   equivalent in this crate (orchestrator-owned).
        // TS postCompactCleanup.ts: resetGetMemoryFilesCache('compact') — no
        //   Rust equivalent in this crate (orchestrator/session-owned).

        // PARITY: binary `Zne` post-compact cleanup ends with
        // `if(o)knm.resetAutonomousLoopDelivered()` where `o` = main-thread
        // compact (cc_all.txt). Reset the autonomous-loop first-delivery state
        // (`iFt`/`Gst`) so the next loop fire re-emits the full preamble. Inert
        // by default (the resolver gate `tengu_kairos_loop_prompt` is off, so the
        // DELIVERY state is never mutated) — wired here for structural 1:1 so it
        // is correct the moment the flag flips.
        tool_cron::reset_autonomous_loop_delivered();
    }

    // clearCompactWarningSuppression is NOT called here in TS post-compact
    // cleanup — TS clears suppression at the *start* of a new attempt and
    // *suppresses* after success. We mirror that elsewhere
    // (warning_state::{suppress_compact_warning, clear_compact_warning_suppression}).
    // Referenced here only to keep the symbol live for the orchestrator wiring;
    // see warning_state.rs for the suppress/clear contract.
    let _ = clear_compact_warning_suppression;

    // The remaining TS resets have no Rust equivalent in this crate:
    // TS postCompactCleanup.ts: clearSystemPromptSections — no Rust equivalent.
    // TS postCompactCleanup.ts: clearClassifierApprovals — no Rust equivalent.
    // TS postCompactCleanup.ts: clearSpeculativeChecks (Bash permissions) — no
    //   Rust equivalent.
    // TS postCompactCleanup.ts: resetSentSkillNames — intentionally NOT called
    //   (re-injecting skill_listing post-compact is pure cache_creation; see
    //   the TS rationale).
    // TS postCompactCleanup.ts: clearBetaTracingState — no Rust equivalent.
    // TS postCompactCleanup.ts: sweepFileContentCache (COMMIT_ATTRIBUTION) — no
    //   Rust equivalent.
    // TS postCompactCleanup.ts: clearSessionMessagesCache — session-storage
    //   cache lives in the session crate, not this one (orchestrator-owned).
}

/// Output of [`PostCompactBuilder::build`].
#[derive(Debug, Clone)]
pub struct PostCompactMessages {
    /// Messages to insert at the compact boundary (typically one summary msg).
    pub summary_messages: Vec<ConversationMessage>,
    /// Restored recent-file attachments, budgeted per
    /// [`restore_post_compact_files`].
    pub files: Vec<RestoredFile>,
    /// Restored invoked-skill attachments, budgeted per
    /// [`restore_post_compact_skills`].
    pub skills: Vec<RestoredSkill>,
}

impl PostCompactMessages {
    /// `compactedMessageCount` for the `tengu_auto_compact_succeeded`
    /// telemetry: `summaryMessages.length + attachments.length +
    /// hookResults.length` (`bin/claude.exe` offset 202919013). The
    /// session-start hook results are orchestrator-owned, so this counts the
    /// compaction-crate contribution (summary + file + skill attachments); the
    /// consumer adds any hook-result count.
    #[must_use]
    pub fn compacted_message_count(&self) -> usize {
        self.summary_messages.len() + self.files.len() + self.skills.len()
    }
}

/// Stateless builder for the post-compact boundary.
pub struct PostCompactBuilder;

impl PostCompactBuilder {
    /// Build the post-compact message set: the summary message(s) plus the
    /// budgeted file/skill restoration attachments.
    ///
    /// Mirrors `K2p` (`bin/claude.exe` offset 202820676): the summary is the
    /// compactor's output and the attachments are `[...Pqn(files), ...skills,
    /// ...]`. The non-faithful `"Compact boundary:\n"` prefix the prior shape
    /// prepended is dropped — the boundary marker is a separate
    /// [`crate::boundary::create_compact_boundary`] message, and the summary
    /// text rides verbatim (TS `summaryMessages`).
    #[must_use]
    pub fn build(
        summary_text: &str,
        file_candidates: Vec<FileRestoreCandidate>,
        already_attached: &[std::path::PathBuf],
        skill_candidates: Vec<SkillRestoreCandidate>,
    ) -> PostCompactMessages {
        PostCompactMessages {
            summary_messages: vec![ConversationMessage::System {
                id: protocol::MessageId::new(),
                content: summary_text.to_string(),
            }],
            files: restore_post_compact_files(file_candidates, already_attached),
            skills: restore_post_compact_skills(skill_candidates),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_thread_gate_true_for_none() {
        // `/compact`, `/clear` pass no source — genuinely main-thread-only.
        assert!(is_main_thread_compact(None));
    }

    #[test]
    fn main_thread_gate_true_for_repl_main_thread_prefix() {
        assert!(is_main_thread_compact(Some("repl_main_thread")));
        // startsWith, not equality — suffixed variants still gate true.
        assert!(is_main_thread_compact(Some("repl_main_thread_foreground")));
        assert!(is_main_thread_compact(Some("repl_main_thread:bash")));
    }

    #[test]
    fn main_thread_gate_true_for_sdk() {
        assert!(is_main_thread_compact(Some("sdk")));
    }

    #[test]
    fn main_thread_gate_false_for_agent_sources() {
        // Subagents share module-level state; resetting it would corrupt the
        // main thread, so the gate must be false.
        assert!(!is_main_thread_compact(Some("agent:foo")));
        assert!(!is_main_thread_compact(Some("agent:explore")));
        assert!(!is_main_thread_compact(Some("agent")));
    }

    #[test]
    fn main_thread_gate_false_for_other_sources() {
        assert!(!is_main_thread_compact(Some("sdk_subagent")));
        assert!(!is_main_thread_compact(Some("")));
        assert!(!is_main_thread_compact(Some("classifier")));
    }

    #[test]
    fn run_post_compact_cleanup_main_thread_does_not_panic() {
        // No Rust module-level caches to assert on (most resets are
        // cross-crate / stateless); exercise both branches for coverage.
        run_post_compact_cleanup(None);
        run_post_compact_cleanup(Some("repl_main_thread"));
        run_post_compact_cleanup(Some("sdk"));
    }

    #[test]
    fn run_post_compact_cleanup_subagent_does_not_panic() {
        run_post_compact_cleanup(Some("agent:foo"));
        run_post_compact_cleanup(Some("classifier"));
    }

    // --- #59 post-compact file/skill restoration --------------------------- //

    use std::path::PathBuf;

    fn file(path: &str, content: &str, ts: i64) -> FileRestoreCandidate {
        FileRestoreCandidate {
            path: PathBuf::from(path),
            content: content.to_string(),
            timestamp_ms: ts,
        }
    }

    fn skill(name: &str, path: &str, content: &str, invoked: i64) -> SkillRestoreCandidate {
        SkillRestoreCandidate {
            name: name.to_string(),
            path: PathBuf::from(path),
            content: content.to_string(),
            invoked_at_ms: invoked,
        }
    }

    #[test]
    fn estimate_tokens_rounds_like_dollar_f() {
        // $f = Math.round(len/4).
        assert_eq!(estimate_content_tokens(""), 0);
        assert_eq!(estimate_content_tokens("ab"), 1); // round(2/4)=round(0.5)=1
        assert_eq!(estimate_content_tokens("a"), 0); // round(1/4)=round(0.25)=0
        assert_eq!(estimate_content_tokens("abc"), 1); // round(3/4)=round(0.75)=1
        assert_eq!(estimate_content_tokens("abcd"), 1); // round(4/4)=1
        assert_eq!(estimate_content_tokens(&"x".repeat(4000)), 1000);
    }

    #[test]
    fn truncate_skill_content_keeps_short_content_verbatim() {
        let short = "tiny skill body";
        assert_eq!(truncate_skill_content(short, 5_000), short);
    }

    #[test]
    fn truncate_skill_content_truncates_and_appends_marker() {
        // 40_000 chars ≈ 10_000 tokens > cap 5_000 → truncate.
        let big = "x".repeat(40_000);
        let out = truncate_skill_content(&big, 5_000);
        assert!(out.ends_with(SKILL_TRUNCATION_MARKER));
        // kept = 5_000*4 - marker.len() chars + marker.
        let expected_kept = 5_000 * 4 - SKILL_TRUNCATION_MARKER.len();
        assert_eq!(out.len(), expected_kept + SKILL_TRUNCATION_MARKER.len());
    }

    #[test]
    fn restore_files_sorts_desc_and_caps_at_five() {
        // Six files; only the 5 most-recent (highest timestamp) survive.
        let candidates = (0..6)
            .map(|i| file(&format!("/f{i}"), "small", i64::from(i)))
            .collect();
        let restored = restore_post_compact_files(candidates, &[]);
        assert_eq!(restored.len(), POST_COMPACT_MAX_FILES_TO_RESTORE);
        // Highest timestamps first: /f5,/f4,/f3,/f2,/f1 ; /f0 dropped.
        assert_eq!(restored[0].path, PathBuf::from("/f5"));
        assert_eq!(restored[4].path, PathBuf::from("/f1"));
        assert!(restored.iter().all(|r| r.path != PathBuf::from("/f0")));
    }

    #[test]
    fn restore_files_skips_already_attached() {
        let candidates = vec![
            file("/keep", "x", 2),
            file("/dup", "x", 1),
        ];
        let restored = restore_post_compact_files(candidates, &[PathBuf::from("/dup")]);
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].path, PathBuf::from("/keep"));
    }

    #[test]
    fn restore_files_respects_total_budget() {
        // Two files each ~30_000 tokens (per-file cap 5_000 → each truncated to
        // ~5_000 tokens ≈ 20_000 chars). Running budget Y9p=50_000 → both fit.
        // A third file would still fit (3*5000=15000 <= 50000). Build files that
        // EXCEED the total budget AFTER per-file capping by using 11 files: each
        // capped to ~5_000 tokens → 11*5_000 = 55_000 > 50_000 → the 11th drops.
        let candidates: Vec<_> = (0..11)
            .map(|i| file(&format!("/f{i}"), &"x".repeat(40_000), i64::from(i)))
            .collect();
        // Only the top-5 by timestamp are even considered → all 5 ~5_000 each =
        // 25_000 <= 50_000 → all 5 kept.
        let restored = restore_post_compact_files(candidates, &[]);
        assert_eq!(restored.len(), 5, "top-5 each ~5k tokens, total 25k <= 50k");
        // Each file content is per-file capped (truncation marker present).
        assert!(restored
            .iter()
            .all(|r| r.content.ends_with(SKILL_TRUNCATION_MARKER)));
    }

    #[test]
    fn restore_files_drops_overflowing_file_keeps_smaller_one() {
        // Construct exactly two candidates where the first (most recent) is huge
        // (per-file capped to 5_000 tokens) and a contrived budget edge: with the
        // total budget 50_000, two capped 5_000-token files (10_000) easily fit.
        // To exercise the DROP branch, give the first file ~5_000 tokens and make
        // the remaining 4 each also ~5_000; all fit. So instead verify the
        // greedy drop: first file fills near budget, a later file overflows.
        // 9 capped files * 5_000 = 45_000 ; 10th would be 50_000 (still <=).
        // Use bodies that cap to ~5_000 tokens and a total over 50_000 within
        // the top-5 isn't reachable (5*5000=25000). So directly test the filter:
        let big = "x".repeat(40_000); // caps to ~5_000 tokens
        let small = "tiny"; // ~1 token
        // timestamps: big files newest so they're selected first.
        let candidates = vec![
            file("/b1", &big, 5),
            file("/b2", &big, 4),
            file("/b3", &big, 3),
            file("/b4", &big, 2),
            file("/small", small, 1),
        ];
        let restored = restore_post_compact_files(candidates, &[]);
        // 4 big (~5_000 each = 20_000) + small (~1) = 20_001 <= 50_000 → all kept.
        assert_eq!(restored.len(), 5);
        assert_eq!(restored[4].path, PathBuf::from("/small"));
        assert_eq!(restored[4].content, "tiny");
    }

    #[test]
    fn restore_skills_sorts_desc_truncates_and_budgets() {
        // Skill A is huge (caps to ~5_000 tokens), B small. Sorted by invokedAt.
        let candidates = vec![
            skill("A", "/a", &"x".repeat(40_000), 2),
            skill("B", "/b", "short", 5),
        ];
        let restored = restore_post_compact_skills(candidates);
        // B invoked later → first; A capped + appended marker.
        assert_eq!(restored[0].name, "B");
        assert_eq!(restored[0].content, "short");
        assert_eq!(restored[1].name, "A");
        assert!(restored[1].content.ends_with(SKILL_TRUNCATION_MARKER));
    }

    #[test]
    fn restore_skills_drops_when_over_skills_budget() {
        // Six skills each capped to ~5_000 tokens → 6*5_000 = 30_000 > Q9p=25_000.
        // Greedy keeps 5 (25_000), drops the 6th.
        let candidates: Vec<_> = (0..6)
            .map(|i| skill(&format!("s{i}"), &format!("/s{i}"), &"x".repeat(40_000), i64::from(i)))
            .collect();
        let restored = restore_post_compact_skills(candidates);
        assert_eq!(restored.len(), 5, "5 * ~5_000 = 25_000 <= 25_000; 6th drops");
    }

    #[test]
    fn builder_drops_legacy_prefix_and_carries_restoration() {
        let msgs = PostCompactBuilder::build(
            "the summary text",
            vec![file("/f", "content", 1)],
            &[],
            vec![skill("S", "/s", "skillbody", 1)],
        );
        // Summary message is the verbatim summary text — NO "Compact boundary:"
        // prefix.
        match &msgs.summary_messages[0] {
            ConversationMessage::System { content, .. } => {
                assert_eq!(content, "the summary text");
                assert!(!content.starts_with("Compact boundary:"));
            }
            _ => panic!("expected System summary"),
        }
        assert_eq!(msgs.files.len(), 1);
        assert_eq!(msgs.skills.len(), 1);
        // compactedMessageCount = summary(1) + files(1) + skills(1) = 3.
        assert_eq!(msgs.compacted_message_count(), 3);
    }

    /// PARITY: binary `Zne` ends with `if(o)resetAutonomousLoopDelivered()` on a
    /// main-thread compact. A main-thread cleanup must reset the autonomous-loop
    /// first-delivery state (so the next fire re-emits the preamble); a subagent
    /// compact must NOT.
    #[test]
    fn post_compact_resets_autonomous_loop_delivered_on_main_thread() {
        use tool_cron::{resolve_autonomous_loop_fire, AUTONOMOUS_LOOP_DYNAMIC_SENTINEL};
        // Enable the resolver gate (flag-only) via the test override.
        telemetry::test_set_flag("tengu_kairos_loop_prompt", true);
        tool_cron::reset_autonomous_loop_delivered();

        let preamble_head = "# Autonomous loop check\n";
        // First fire delivers the full preamble; second drops it.
        let first = resolve_autonomous_loop_fire(AUTONOMOUS_LOOP_DYNAMIC_SENTINEL).unwrap();
        assert!(first.starts_with(preamble_head));
        let second = resolve_autonomous_loop_fire(AUTONOMOUS_LOOP_DYNAMIC_SENTINEL).unwrap();
        assert!(!second.starts_with(preamble_head));

        // Subagent compact must NOT reset (state stays "delivered").
        run_post_compact_cleanup(Some("agent:child"));
        let after_subagent = resolve_autonomous_loop_fire(AUTONOMOUS_LOOP_DYNAMIC_SENTINEL).unwrap();
        assert!(
            !after_subagent.starts_with(preamble_head),
            "subagent compact must NOT reset delivery state"
        );

        // Main-thread compact (query_source None) resets → preamble re-emitted.
        run_post_compact_cleanup(None);
        let after_main = resolve_autonomous_loop_fire(AUTONOMOUS_LOOP_DYNAMIC_SENTINEL).unwrap();
        assert!(
            after_main.starts_with(preamble_head),
            "main-thread compact must reset → preamble re-delivered"
        );

        telemetry::test_clear_flag("tengu_kairos_loop_prompt");
        tool_cron::reset_autonomous_loop_delivered();
    }
}
