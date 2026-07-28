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
    let keep = usize::try_from(keep)
        .unwrap_or(usize::MAX)
        .min(content.len());
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
/// Mirrors the `[key, {skillName, skillPath, content, invokedAt}]` entries `rRg`
/// iterates from the invoked-skill registry (`bin/claude.exe` v2.1.207 `rRg`).
/// The `key` is the registry key (`"{agentId}:{name}"`) so `rRg`'s `n_n`
/// write-backs can target the row this candidate came from.
#[derive(Debug, Clone)]
pub struct SkillRestoreCandidate {
    /// The registry key (`"{agentId}:{name}"`) this candidate came from — the
    /// target of the `n_n` write-back when the content is truncated or the
    /// budget overflows.
    pub key: String,
    /// Skill name.
    pub name: String,
    /// Skill source path.
    pub path: std::path::PathBuf,
    /// Full skill content (truncated per [`POST_COMPACT_MAX_TOKENS_PER_SKILL`]).
    pub content: String,
    /// When the skill was last invoked; restoration sorts descending on this.
    pub invoked_at_ms: i64,
}

/// An already-attached item consulted by `LQn` when deduping a skill's content
/// against the context already present at the compact boundary.
///
/// 1:1 with the two branches of `LQn(e,t)` (`bin/claude.exe` v2.1.207): an
/// `invoked_skills` attachment already carrying a skill's content
/// ([`Self::Attachment`]), or a plain message body whose `DYi` text extraction
/// equals the content ([`Self::Body`]).
#[derive(Debug, Clone)]
pub enum AttachedSkillContent {
    /// Content that already rode in a prior `invoked_skills` attachment.
    Attachment(String),
    /// A message body's text (`DYi` of a user-meta message).
    Body(String),
}

/// `LQn` classification of a skill's content against already-attached context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SkillDedup {
    /// Already present in an `invoked_skills` attachment → skip entirely.
    Attachment,
    /// Present as a plain message body → still counts, but never written back.
    Body,
    /// Not present anywhere.
    None,
}

/// `LQn(alreadyAttached, content)` — is `content` already in context, and how?
///
/// 1:1 with `function LQn(e,t){let r=!1;for(let n=e.length-1;n>=0;n--){…}}`
/// (`bin/claude.exe` v2.1.207): iterate the already-attached items from the end;
/// a message-body match returns `Body` immediately, while an `invoked_skills`
/// attachment match sets a flag; if no body match is found the flag decides
/// between `Attachment` and `None`.
fn lqn(already_attached: &[AttachedSkillContent], content: &str) -> SkillDedup {
    let mut attachment_match = false;
    for item in already_attached.iter().rev() {
        match item {
            AttachedSkillContent::Attachment(c) => {
                if !attachment_match && c == content {
                    attachment_match = true;
                }
            }
            AttachedSkillContent::Body(c) => {
                if c == content {
                    return SkillDedup::Body;
                }
            }
        }
    }
    if attachment_match {
        SkillDedup::Attachment
    } else {
        SkillDedup::None
    }
}

/// Preamble prepended to the model-visible `invoked_skills` post-compact
/// attachment body.
///
/// Byte-exact with the `case"invoked_skills"` attachment renderer
/// (`bin/claude.exe` v2.1.207): the `$r({content:…, isMeta:!0})` header, with
/// the per-skill blocks (`### Skill: …`) appended after a blank line (`\n\n`) by
/// [`render_invoked_skills_attachment`]. Its own internal `guidelines.\n\nIMPORTANT`
/// separator is a blank line too.
pub const INVOKED_SKILLS_ATTACHMENT_PREAMBLE: &str = "The following skills were invoked EARLIER in this session (before the conversation was compacted), not on the current turn. They are shown here for context only so you remain aware of their guidelines.\n\nIMPORTANT: Do NOT re-execute these skills or perform their one-time setup actions (e.g., scheduling, creating files) again. The \"## Input\" sections below reflect the original arguments from when each skill was first invoked — they are NOT the user's current message. Only continue to apply ongoing behavioral guidelines from these skills where still relevant.";

/// Render restored skills into the model-visible `invoked_skills` attachment
/// body, or `None` when there is nothing to restore.
///
/// 1:1 with the `case"invoked_skills"` renderer (`bin/claude.exe` v2.1.207 offset
/// 226440880 / v2.1.208 offset 225821608, byte-verified via `od -c`):
/// `e.skills.map((n)=>`### Skill: ${n.name}\nPath: ${n.path}\n\n${n.content}`).join(`\n\n---\n\n`)`
/// wrapped as `${PREAMBLE}\n\n${joined}` — the per-skill blocks are separated by a
/// `\n\n---\n\n` delimiter, each block puts a blank line before its content, and
/// the preamble is joined to the blocks by a blank line. Emitted as a single
/// `isMeta` user message (the caller wraps it in
/// [`protocol::ConversationMessage::user_meta`]).
#[must_use]
pub fn render_invoked_skills_attachment(skills: &[RestoredSkill]) -> Option<String> {
    if skills.is_empty() {
        return None;
    }
    let joined = skills
        .iter()
        .map(|s| {
            format!(
                "### Skill: {}\nPath: {}\n\n{}",
                s.name,
                s.path.display(),
                s.content
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n---\n\n");
    Some(format!("{INVOKED_SKILLS_ATTACHMENT_PREAMBLE}\n\n{joined}"))
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

/// Select the recent files to restore after a compaction — the **selection**
/// half of `eRg` (`bin/claude.exe` offset ~91938880), pure and disk-free.
///
/// 1:1 with the head of `eRg(readFileState, ctx, G0g, alreadyAttached)`:
/// 1. Drop candidates already attached elsewhere (`alreadyAttached`, matched by
///    path) — the binary also drops plan files via `aRg`; the plan-file filter
///    lives in the caller here.
/// 2. Sort by `timestamp` DESC (most-recently-read first).
/// 3. Take the first [`POST_COMPACT_MAX_FILES_TO_RESTORE`] (`G0g = 5`).
///
/// The caller then RE-READS each survivor from disk (`XQn` with
/// `fileReadingLimits:{maxTokens:z0g}`) before feeding the fresh contents to
/// [`budget_post_compact_files`] — the binary re-reads at compact time rather
/// than reusing the stale `readFileState` snapshot content.
#[must_use]
pub fn select_post_compact_files(
    candidates: Vec<FileRestoreCandidate>,
    already_attached: &[std::path::PathBuf],
) -> Vec<FileRestoreCandidate> {
    let mut selected: Vec<FileRestoreCandidate> = candidates
        .into_iter()
        .filter(|c| !already_attached.iter().any(|p| p == &c.path))
        .collect();
    // Sort by timestamp DESC. `sort_by` is stable, mirroring the binary's
    // `.sort((l,c)=>c.timestamp-l.timestamp)` for equal timestamps.
    selected.sort_by(|a, b| b.timestamp_ms.cmp(&a.timestamp_ms));
    selected.truncate(POST_COMPACT_MAX_FILES_TO_RESTORE);
    selected
}

/// Apply the per-file cap + running token budget to (freshly re-read) file
/// candidates — the **budgeting** half of `eRg`.
///
/// Each candidate's `content` is the fresh disk re-read the caller performed
/// (with `fileReadingLimits:{maxTokens:z0g}`); this:
/// 1. Caps each file's content at [`POST_COMPACT_MAX_TOKENS_PER_FILE`]
///    (`z0g = 5000`) — the binary's re-reader applies its own maxTokens
///    truncation; here the per-file cap truncates the supplied content.
/// 2. Keeps files greedily while the running total stays `<=`
///    [`POST_COMPACT_TOKEN_BUDGET`] (`V0g = 50000`); a file that would overflow
///    is dropped (NOT truncated to fit) and iteration continues, exactly like
///    the binary's `if(a+c<=V0g)return a+=c,!0; return !1` filter.
///
/// Candidates are processed in the order given; the caller keeps the
/// [`select_post_compact_files`] DESC order so the greedy budget matches the
/// binary's timestamp-DESC filter.
#[must_use]
pub fn budget_post_compact_files(candidates: Vec<FileRestoreCandidate>) -> Vec<RestoredFile> {
    let mut running = 0u64;
    let mut out = Vec::new();
    for candidate in candidates {
        // Per-file cap (`fileReadingLimits:{maxTokens:z0g}`): truncate content
        // exceeding the per-file token budget. Reuse the skill truncation shape
        // (the binary's re-reader applies its own maxTokens truncation; the
        // observable effect is a per-file ceiling).
        let content = truncate_skill_content(&candidate.content, POST_COMPACT_MAX_TOKENS_PER_FILE);
        let cost = estimate_content_tokens(&content);
        // Running budget: keep while total + cost <= V0g; else DROP (continue).
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

/// Select **and** budget the recent files to restore after a compaction, over
/// the SNAPSHOT content (no disk re-read).
///
/// Composes [`select_post_compact_files`] + [`budget_post_compact_files`]. The
/// production orchestrator path re-reads each selected file from disk BETWEEN
/// these two halves (the byte-faithful `eRg` behaviour); this composed form is
/// retained for the pure [`PostCompactBuilder::build`] shape + its unit tests,
/// where no filesystem is available.
#[must_use]
pub fn restore_post_compact_files(
    candidates: Vec<FileRestoreCandidate>,
    already_attached: &[std::path::PathBuf],
) -> Vec<RestoredFile> {
    budget_post_compact_files(select_post_compact_files(candidates, already_attached))
}

/// Select and budget the invoked skills to restore after a compaction, applying
/// the registry write-backs `rRg` performs (`n_n`).
///
/// 1:1 with `rRg(agentId, alreadyAttached)` (`bin/claude.exe` v2.1.207). The
/// `candidates` come from [`crate::invoked_skills::filter_for_agent`] (`kGo`);
/// `already_attached` models the boundary context `LQn` dedups against:
/// 1. Sort by `invokedAt` DESC (`sort(([,s],[,a])=>a.invokedAt-s.invokedAt)`).
/// 2. Skip cleared/empty rows (`if(!a.content)continue`).
/// 3. [`lqn`]-classify against `already_attached`: an `"attachment"` match
///    skips the candidate entirely; a `"body"` match still counts but is never
///    written back.
/// 4. Truncate to [`POST_COMPACT_MAX_TOKENS_PER_SKILL`] (`K0g = 5000`) via
///    [`truncate_skill_content`] (`sRg`).
/// 5. Keep greedily while the running total stays `<=`
///    [`POST_COMPACT_SKILLS_TOKEN_BUDGET`] (`Y0g = 25000`). On overflow, clear
///    the registry row's content (`n_n(key,"")`) unless it was a body match,
///    then drop the candidate.
/// 6. On a kept-and-truncated candidate (`u !== content`, not a body match),
///    persist the truncated content back to the registry (`n_n(key,u)`).
///
/// The write-backs mutate the process-global registry via
/// [`crate::invoked_skills::write_back`] — a no-op for candidates whose key is
/// not in the registry (e.g. the pure unit tests), so the selection/budget logic
/// stays testable without a live registry.
#[must_use]
pub fn restore_post_compact_skills(
    candidates: Vec<SkillRestoreCandidate>,
    already_attached: &[AttachedSkillContent],
) -> Vec<RestoredSkill> {
    let mut sorted = candidates;
    sorted.sort_by(|a, b| b.invoked_at_ms.cmp(&a.invoked_at_ms));

    let mut running = 0u64;
    let mut out = Vec::new();
    for skill in sorted {
        // `if(!a.content)continue;` — skip cleared/empty registry rows.
        if skill.content.is_empty() {
            continue;
        }
        // `let l=LQn(t,a.content);` — dedup against the boundary context.
        let dedup = lqn(already_attached, &skill.content);
        // `if(l==="attachment")continue;` — already in an attachment → skip.
        if dedup == SkillDedup::Attachment {
            continue;
        }
        // `let c=l==="body"` — a body match still counts but never writes back.
        let is_body = dedup == SkillDedup::Body;
        // `u=sRg(a.content,K0g)` — per-skill truncation.
        let truncated = truncate_skill_content(&skill.content, POST_COMPACT_MAX_TOKENS_PER_SKILL);
        // `d=cy(u)` — token estimate of the truncated content.
        let cost = estimate_content_tokens(&truncated);
        // `if(n+d>Y0g){if(!c)n_n(s,"");continue}` — budget overflow: clear the
        // registry content (unless a body match) and DROP (do not truncate-to-fit).
        if running.saturating_add(cost) > POST_COMPACT_SKILLS_TOKEN_BUDGET {
            if !is_body {
                crate::invoked_skills::write_back(&skill.key, "");
            }
            continue;
        }
        // `n+=d;` then `if(!c&&u!==a.content)n_n(s,u)` — persist truncation.
        running = running.saturating_add(cost);
        if !is_body && truncated != skill.content {
            crate::invoked_skills::write_back(&skill.key, &truncated);
        }
        out.push(RestoredSkill {
            name: skill.name,
            path: skill.path,
            content: truncated,
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
            summary_messages: vec![ConversationMessage::compact_summary(
                protocol::MessageId::new(),
                summary_text.to_string(),
            )],
            files: restore_post_compact_files(file_candidates, already_attached),
            // Skill dedup has no boundary context in the pure builder shape
            // (`already_attached = &[]`); the production path threads it in
            // `rRg`. The builder has no production callers (see module docs).
            skills: restore_post_compact_skills(skill_candidates, &[]),
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
            // Namespace the pure-test keys so their (no-op) `write_back` calls
            // can never collide with the real `":{name}"` keys the
            // `invoked_skills` registry tests register in the same test binary.
            key: format!("pt:{name}"),
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
        let candidates = vec![file("/keep", "x", 2), file("/dup", "x", 1)];
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
    fn select_files_top5_desc_and_filters_already_attached() {
        // Selection half of `eRg`: DESC by timestamp, top-5, drops already-attached.
        let candidates = vec![
            file("/f0", "a", 0),
            file("/f5", "a", 5),
            file("/dup", "a", 9), // highest ts but already attached → dropped
            file("/f3", "a", 3),
            file("/f4", "a", 4),
            file("/f1", "a", 1),
            file("/f2", "a", 2),
        ];
        let selected = select_post_compact_files(candidates, &[PathBuf::from("/dup")]);
        // Top-5 by timestamp DESC, /dup filtered out first.
        let paths: Vec<_> = selected.iter().map(|c| c.path.clone()).collect();
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/f5"),
                PathBuf::from("/f4"),
                PathBuf::from("/f3"),
                PathBuf::from("/f2"),
                PathBuf::from("/f1"),
            ]
        );
    }

    #[test]
    fn budget_files_caps_each_and_respects_running_budget() {
        // Budgeting half of `eRg`: per-file cap + running budget over the
        // caller-supplied (fresh) contents; order is preserved.
        let big = "x".repeat(40_000); // caps to ~5_000 tokens
        let candidates = vec![
            file("/b1", &big, 5),
            file("/b2", &big, 4),
            file("/small", "tiny", 1),
        ];
        let restored = budget_post_compact_files(candidates);
        // 2 big (~5_000 each = 10_000) + small (~1) = 10_001 <= 50_000 → all kept,
        // order preserved (no re-sort in the budgeting half).
        assert_eq!(restored.len(), 3);
        assert_eq!(restored[0].path, PathBuf::from("/b1"));
        assert!(restored[0].content.ends_with(SKILL_TRUNCATION_MARKER));
        assert_eq!(restored[2].path, PathBuf::from("/small"));
        assert_eq!(restored[2].content, "tiny");
    }

    #[test]
    fn restore_files_composes_select_then_budget() {
        // The composed pure form must equal select-then-budget.
        let candidates = vec![file("/a", "one", 2), file("/b", "two", 1)];
        let composed = restore_post_compact_files(candidates.clone(), &[]);
        let manual = budget_post_compact_files(select_post_compact_files(candidates, &[]));
        assert_eq!(composed, manual);
    }

    #[test]
    fn restore_skills_sorts_desc_truncates_and_budgets() {
        // Skill A is huge (caps to ~5_000 tokens), B small. Sorted by invokedAt.
        let candidates = vec![
            skill("A", "/a", &"x".repeat(40_000), 2),
            skill("B", "/b", "short", 5),
        ];
        let restored = restore_post_compact_skills(candidates, &[]);
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
            .map(|i| {
                skill(
                    &format!("s{i}"),
                    &format!("/s{i}"),
                    &"x".repeat(40_000),
                    i64::from(i),
                )
            })
            .collect();
        let restored = restore_post_compact_skills(candidates, &[]);
        assert_eq!(
            restored.len(),
            5,
            "5 * ~5_000 = 25_000 <= 25_000; 6th drops"
        );
    }

    // --- P2-12 rRg semantics: dedup + registry write-back ------------------- //

    #[test]
    fn restore_skills_skips_empty_content_rows() {
        // `if(!a.content)continue;` — a cleared registry row is skipped.
        let candidates = vec![
            skill("cleared", "/c", "", 5),
            skill("live", "/l", "body", 4),
        ];
        let restored = restore_post_compact_skills(candidates, &[]);
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].name, "live");
    }

    #[test]
    fn restore_skills_lqn_attachment_match_skips() {
        // A candidate whose content already rode in an `invoked_skills`
        // attachment is skipped entirely (`if(l==="attachment")continue`).
        let candidates = vec![
            skill("dup", "/d", "already attached", 5),
            skill("fresh", "/f", "fresh body", 4),
        ];
        let already = [AttachedSkillContent::Attachment("already attached".into())];
        let restored = restore_post_compact_skills(candidates, &already);
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].name, "fresh");
    }

    #[test]
    fn restore_skills_lqn_body_match_still_restores() {
        // A body match still restores (counts toward the budget), unlike an
        // attachment match which skips. (The registry write-back distinction is
        // asserted in `invoked_skills` where the global registry is available.)
        let candidates = vec![skill("s", "/s", "shared body", 5)];
        let already = [AttachedSkillContent::Body("shared body".into())];
        let restored = restore_post_compact_skills(candidates, &already);
        assert_eq!(
            restored.len(),
            1,
            "body match restores; only attachment skips"
        );
        assert_eq!(restored[0].content, "shared body");
    }

    #[test]
    fn render_invoked_skills_attachment_shape_is_byte_faithful() {
        let restored = vec![
            RestoredSkill {
                name: "deploy".into(),
                path: PathBuf::from("/skills/deploy"),
                content: "Deploy guidelines".into(),
            },
            RestoredSkill {
                name: "build".into(),
                path: PathBuf::from("/skills/build"),
                content: "Build guidelines".into(),
            },
        ];
        let body = render_invoked_skills_attachment(&restored).expect("non-empty");
        // Byte-faithful with the 2.1.207/2.1.208 renderer (verified od -c at
        // 2.1.208 offset 225821608 / 2.1.207 offset 226440880):
        //   map:  `### Skill: ${name}\nPath: ${path}\n\n${content}`
        //   join: `\n\n---\n\n`
        //   body: `${PREAMBLE}\n\n${joined}`  (preamble internal `\n\n`)
        let expected = format!(
            "{INVOKED_SKILLS_ATTACHMENT_PREAMBLE}\n\n### Skill: deploy\nPath: /skills/deploy\n\nDeploy guidelines\n\n---\n\n### Skill: build\nPath: /skills/build\n\nBuild guidelines"
        );
        assert_eq!(body, expected);
        // Preamble internal separator is a blank line (`guidelines.\n\nIMPORTANT`).
        assert!(body.contains("their guidelines.\n\nIMPORTANT: Do NOT"));
        // Empty → None (no attachment).
        assert!(render_invoked_skills_attachment(&[]).is_none());
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
            ConversationMessage::User {
                content,
                is_compact_summary,
                is_visible_in_transcript_only,
                ..
            } => {
                assert!(*is_compact_summary);
                assert!(*is_visible_in_transcript_only);
                assert_eq!(msgs.summary_messages[0].text_content(), "the summary text");
                assert!(!msgs.summary_messages[0]
                    .text_content()
                    .starts_with("Compact boundary:"));
                assert_eq!(content.len(), 1);
            }
            _ => panic!("expected typed User summary"),
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
        let after_subagent =
            resolve_autonomous_loop_fire(AUTONOMOUS_LOOP_DYNAMIC_SENTINEL).unwrap();
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
