//! Rate-limit resume checkpoint — the `RESUME.md` document and its vocabulary.
//!
//! SC-02. New in 2.1.238 (`performRateLimitCheckpoint` has 6 hits there and
//! **0** in 2.1.220): when a session hits — or nears — its usage limit, the
//! oracle snapshots the working tree into a detached WIP commit under a private
//! ref, writes a human-readable `RESUME.md` explaining how to pick the session
//! back up, and hands the user the ref that holds their in-progress files.
//!
//! Oracle module `eal` (cc-238.js @292197753) exports exactly:
//!
//! ```text
//! CHECKPOINT_REF_PREFIX, MAX_CHECKPOINT_FILE_COUNT, MAX_CHECKPOINT_TOTAL_BYTES,
//! RESUME_MD_REPO_PATH, clearLastCheckpointResult, getLastCheckpointResult,
//! performRateLimitCheckpoint, useRateLimitCheckpointResult
//! ```
//!
//! # What is ported here, and what is not
//!
//! **Ported:** the whole *document* half — the byte-exact `RESUME.md` renderer
//! (`K4v` @292206132), its todo-line sanitiser (`Zsl` @292197365), the four
//! module constants, the trigger vocabulary and the complete 14-value
//! skip-reason taxonomy that `getLastCheckpointResult` persists. This is the
//! user-visible surface and the part that can be pinned by test.
//!
//! **Deliberately NOT ported here:** the git executor `q4v` (@292197916) and
//! its trigger wiring. `q4v` is ~200 lines of git *plumbing* whose entire value
//! is its safety envelope, and every step of it either writes into the user's
//! repository or decides not to:
//!
//! ```text
//! gate  Vs("allow_local_checkpoint_commit"); skip when non-interactive / remote workspace
//! stat  git root and git dir must both be canonically contained (`Qsl`), the
//!       git dir must have no `commondir`, and none of objects/refs/refs/claude/
//!       logs/logs/refs/logs/refs/claude/packed-refs/reftable — nor any entry of
//!       objects/ — may be a symlink
//! env   GIT_COMMON_DIR, GIT_WORK_TREE, GIT_ALLOW_PROTOCOL=none, GIT_NO_LAZY_FETCH=1,
//!       GIT_NO_REPLACE_OBJECTS=1, GIT_TERMINAL_PROMPT=0, and a PRIVATE
//!       GIT_INDEX_FILE (`<gitdir>/claude-checkpoint-index.<pid>`) so the user's
//!       own index is never touched
//! skip  sparse checkout (core.sparseCheckout=true), LFS / `filter=lfs` in
//!       .gitattributes, an in-progress sequencer (MERGE_HEAD, CHERRY_PICK_HEAD,
//!       REVERT_HEAD, BISECT_LOG, rebase-merge, rebase-apply), no HEAD
//! build read-tree HEAD → ls-files -z --cached + ls-files -z -o --exclude-standard
//!       → per-path lstat (paths containing \n, \r, `"`, `\` or a `.`/`..`
//!       segment are dropped; non-files dropped; uncontained realpaths dropped;
//!       mode 100755 when the owner-execute bit is set, else 100644) under the
//!       25 000-path and 2 GiB caps → hash-object -w --no-filters --stdin-paths
//!       → update-index --add --index-info (+ --force-remove for vanished cached
//!       paths) → write-tree → commit-tree -p HEAD
//! ref   update-ref --no-deref under core.logAllRefUpdates=false, then append
//!       `/<RESUME_MD_REPO_PATH>` to info/exclude and GC sibling checkpoint refs
//!       older than STALE_CHECKPOINT_REF_AGE
//! ```
//!
//! Half of that envelope is the feature: a checkpoint that skips one of the
//! containment or symlink checks writes into a repository it was supposed to
//! leave alone, and a `hash-object`/`update-index` pipeline that mis-handles the
//! path filter commits files the user excluded. It also has no trigger to hang
//! from in this crate — the near-limit / rate-limited edges live in the
//! orchestrator's turn loop, outside `session`. So the executor is left as one
//! coherent, reviewable follow-up against the recipe above rather than landed
//! half-checked; nothing in this module is load-bearing for it beyond supplying
//! the document, the constants and the reasons it must report.

use engine::session::{TodoItem, TodoState};
use once_cell::sync::Lazy;
use regex::Regex;

/// `oDi = "refs/claude/checkpoint-"` — the ref namespace a checkpoint commit is
/// parked under, one ref per session (`<prefix><first 8 chars of session id>`).
///
/// Rebranded to the `refs/lingxi/` namespace. Distinct from `local-apps`'
/// `refs/lingxi/checkpoints` (plural, a directory of refs) — the two cannot
/// collide.
pub const CHECKPOINT_REF_PREFIX: &str = "refs/lingxi/checkpoint-";

/// `nDi = ".claude/RESUME.md"` — repo-relative path of the resume document,
/// rebranded onto [`branding::DOT_DIR`]. It is written to the working tree AND
/// hashed into the checkpoint tree at this exact path, so the two must agree.
pub const RESUME_MD_REPO_PATH: &str = ".lingxi/RESUME.md";

/// `v6f = 25000` — combined cap on cached + untracked paths. Above it the
/// checkpoint is skipped as [`CheckpointSkipReason::TooLarge`].
pub const MAX_CHECKPOINT_FILE_COUNT: usize = 25_000;

/// `T6f = 2147483648` — 2 GiB cap on the total content hashed into one
/// checkpoint. Above it the checkpoint is skipped as
/// [`CheckpointSkipReason::TooLarge`].
pub const MAX_CHECKPOINT_TOTAL_BYTES: u64 = 2_147_483_648;

/// `xxe = 30000` — timeout, in milliseconds, for each git subprocess
/// (`hash-object --stdin-paths` gets `4 * xxe`).
pub const GIT_TIMEOUT_MS: u64 = 30_000;

/// `j4v = 1209600` — 14 days, in seconds. A sibling checkpoint ref whose commit
/// is older than this is deleted after a successful checkpoint.
pub const STALE_CHECKPOINT_REF_AGE_SECS: u64 = 1_209_600;

/// `b6f = 500` — todo lines are sanitised, then truncated to this many UTF-16
/// code units plus `…`.
const TODO_LINE_MAX_UTF16: usize = 500;

/// Number of leading session-id characters that name the ref (`o.slice(0,8)`).
const REF_SESSION_ID_PREFIX_LEN: usize = 8;

/// Why the checkpoint ran.
///
/// The wire spellings (`near_limit` / `rate_limited`) go to telemetry and to the
/// persisted result; the rendered spellings (`near-limit` / `rate-limited`) go
/// into the `Trigger:` line of the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointTrigger {
    /// `"near_limit"` — the session is approaching its usage limit.
    NearLimit,
    /// `"rate_limited"` — the request was already refused.
    RateLimited,
}

impl CheckpointTrigger {
    /// The wire spelling (telemetry `trigger` property, persisted result).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NearLimit => "near_limit",
            Self::RateLimited => "rate_limited",
        }
    }

    /// The rendered spelling for the `Trigger:` line —
    /// `e.trigger==="near_limit"?"near-limit":"rate-limited"`.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::NearLimit => "near-limit",
            Self::RateLimited => "rate-limited",
        }
    }
}

/// Every reason `q4v` can decline to write a checkpoint, in the order the
/// oracle tests them. All 14 are persisted through `getLastCheckpointResult`
/// and reported to `usage_limit_checkpoint_commit`, so the spellings are wire
/// values, not prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointSkipReason {
    /// `Dn()` — running non-interactively (print mode, SDK, hooks).
    NonInteractive,
    /// `Ca()` — the workspace is remote, so a local commit means nothing.
    RemoteWorkspace,
    /// `!Vs("allow_local_checkpoint_commit")` — gate off (policy / config).
    Policy,
    /// The cwd is not inside a git work tree, or the git dir cannot be resolved.
    NotGit,
    /// `VXt()!==!1` — a bare repository has no working tree to snapshot.
    BareRepo,
    /// The work-tree root does not canonicalise, or it *contains* `$HOME`
    /// (`Qsl(home, root)`) — snapshotting a repo rooted at or above the home
    /// directory is refused.
    GitRootUncontained,
    /// The git dir does not canonicalise, is not contained in the work tree,
    /// has a `commondir` (a linked worktree), or one of its well-known
    /// entries — including any entry of `objects/` — is a symlink.
    GitDirUncontained,
    /// `core.sparseCheckout=true`: the working tree is not the whole tree.
    SparseCheckout,
    /// A `lfs` git-dir directory, or `filter=lfs` in `.gitattributes` — content
    /// filters would rewrite what is hashed.
    ContentFilters,
    /// `MERGE_HEAD`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`, `BISECT_LOG`,
    /// `rebase-merge` or `rebase-apply` is present.
    SequencerInProgress,
    /// `rev-parse --verify HEAD` failed — an unborn branch has no parent to
    /// commit against.
    NoHead,
    /// Above [`MAX_CHECKPOINT_FILE_COUNT`] paths or
    /// [`MAX_CHECKPOINT_TOTAL_BYTES`] of content.
    TooLarge,
    /// Writing `RESUME.md` was refused (symlink/parent-dir guard, or an I/O
    /// error).
    ResumeWriteRefused,
    /// Any git subprocess exited non-zero, timed out, or produced output the
    /// pipeline could not use (e.g. `hash-object --stdin-paths` returning a
    /// different number of hashes than paths given).
    GitError,
}

impl CheckpointSkipReason {
    /// The wire spelling persisted into the checkpoint result and reported to
    /// `usage_limit_checkpoint_commit`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NonInteractive => "non_interactive",
            Self::RemoteWorkspace => "remote_workspace",
            Self::Policy => "policy",
            Self::NotGit => "not_git",
            Self::BareRepo => "bare_repo",
            Self::GitRootUncontained => "gitroot_uncontained",
            Self::GitDirUncontained => "gitdir_uncontained",
            Self::SparseCheckout => "sparse_checkout",
            Self::ContentFilters => "content_filters",
            Self::SequencerInProgress => "sequencer_in_progress",
            Self::NoHead => "no_head",
            Self::TooLarge => "too_large",
            Self::ResumeWriteRefused => "resume_write_refused",
            Self::GitError => "git_error",
        }
    }
}

/// The outcome `performRateLimitCheckpoint` resolves to and latches:
/// `{committed:!0,ref,resumePath,commitSha}` or
/// `{committed:!1,skipReason}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckpointResult {
    /// A checkpoint commit was written.
    Committed {
        /// The ref now pointing at it (`<CHECKPOINT_REF_PREFIX><sid8>`).
        ref_name: String,
        /// Repo-relative path of the written document
        /// ([`RESUME_MD_REPO_PATH`]).
        resume_path: &'static str,
        /// The commit the ref points at.
        commit_sha: String,
    },
    /// Nothing was written, for this reason.
    Skipped(CheckpointSkipReason),
}

/// The ref for `session_id` — `` `${oDi}${o.slice(0,8)}` ``.
///
/// Sliced by `char`, not by byte, so a non-ASCII id can never split a code
/// point; real session ids are UUIDs, where the two agree.
#[must_use]
pub fn checkpoint_ref(session_id: &str) -> String {
    let short: String = session_id.chars().take(REF_SESSION_ID_PREFIX_LEN).collect();
    format!("{CHECKPOINT_REF_PREFIX}{short}")
}

/// The `RESUME.md` inputs (`K4v`'s single argument).
#[derive(Debug, Clone, Copy)]
pub struct ResumeDoc<'a> {
    /// The session to resume.
    pub session_id: &'a str,
    /// The ref holding the snapshot ([`checkpoint_ref`]).
    pub ref_name: &'a str,
    /// Why the checkpoint ran.
    pub trigger: CheckpointTrigger,
    /// `TodoWrite` state at checkpoint time, in list order.
    pub todos: &'a [TodoItem],
    /// `new Date().toISOString()` for the `Written:` line — injected so the
    /// renderer stays pure. [`now_iso`] produces the oracle's format.
    pub written_iso: &'a str,
}

/// `new Date().toISOString()` — `2026-08-20T15:08:27.000Z`.
#[must_use]
pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Control / bidi / zero-width characters `Zsl` collapses to a single space.
///
/// The oracle class, written out in escapes:
/// `[\x00-\x1f\x7f-\x9f\u061c\u200b-\u200f\u2028-\u202e\u2066-\u2069\ufeff]+`
/// — note the `+`: a RUN collapses to ONE space, not one space per character.
static TODO_LINE_UNSAFE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"[\x00-\x1F\x7F-\x9F\x{061C}\x{200B}-\x{200F}\x{2028}-\x{202E}\x{2066}-\x{2069}\x{FEFF}]+",
    )
    .expect("static todo-line sanitiser regex")
});

/// `Zsl(e)` (@292197365) — make one todo safe to paste into a markdown list.
///
/// ```js
/// function Zsl(e){ let t=e.replace(/…/g," ");
///   return t.length>b6f?`${wo(t,b6f)}…`:t }
/// ```
///
/// Newlines are part of the class, so a multi-line todo becomes one line and
/// can never forge a second list item; the length test is on the ALREADY
/// sanitised text, in UTF-16 code units.
#[must_use]
pub fn sanitize_todo_line(text: &str) -> String {
    let collapsed = TODO_LINE_UNSAFE.replace_all(text, " ").into_owned();
    // `wo(t,b6f)` slices to 500 UTF-16 units (dropping a trailing lone high
    // surrogate) and does NOT trim; only then is the ellipsis appended.
    // Bound in its own statement so the borrow ends before the `None` arm
    // moves `collapsed` out.
    let truncated = truncate_utf16_units(&collapsed, TODO_LINE_MAX_UTF16);
    match truncated {
        Some(head) => format!("{head}\u{2026}"),
        None => collapsed,
    }
}

/// `wo(e,t)` (@281366731) — the head of `text`, at most `max_units` UTF-16 code
/// units, or `None` when `text` already fits (`if(e.length<=t)return e`).
///
/// ```js
/// function wo(e,t){ if(t<=0)return""; if(e.length<=t)return e;
///   let r=e.slice(0,t),n=r.charCodeAt(t-1);
///   return cTu(n>=55296&&n<=56319?r.slice(0,-1):r) }
/// ```
///
/// The surrogate test is why this cannot be a `char` count: JS slices UTF-16
/// units, so a cut that lands between the halves of a non-BMP character drops
/// the orphaned high surrogate rather than emitting a lone one. (`cTu` is a
/// UTF-16 round-trip, a no-op on the well-formed result — and unrepresentable
/// in Rust, where a `String` cannot hold a lone surrogate anyway.)
fn truncate_utf16_units(text: &str, max_units: usize) -> Option<String> {
    let units: Vec<u16> = text.encode_utf16().collect();
    if units.len() <= max_units {
        return None;
    }
    let mut head = &units[..max_units];
    if let Some(&last) = head.last() {
        if (0xD800..=0xDBFF).contains(&last) {
            head = &head[..head.len() - 1];
        }
    }
    Some(String::from_utf16_lossy(head))
}

/// Render `RESUME.md` — `K4v` (@292206132), line for line.
///
/// ```js
/// let t=["# Claude Code — resume checkpoint","",`Session: ${e.sessionId}`,
///        `Written: ${new Date().toISOString()}`,
///        `Trigger: ${e.trigger==="near_limit"?"near-limit":"rate-limited"}`,
///        `Ref with your in-progress files: ${e.ref}`,"","## To resume","",
///        `    claude --resume ${e.sessionId}`,"",
///        "(or open Claude Code in this directory and run /resume)","",
///        "## Plan (from TodoWrite state)",""];
/// ```
///
/// Rebrands, per repo precedent: the product noun is [`branding::PRODUCT_NAME`]
/// (as in `engine::settings::enterprise`) and the invocation is
/// `lingxi --resume <id>` — the exact spelling the port's own
/// "Session … saved. Resume with:" hint already prints
/// (`apps/cli/src/mode.rs`). Every other byte is the oracle's.
///
/// Two details worth not "tidying":
///
/// * `## What's next` and its answer are inside the non-empty-todos branch. An
///   empty list emits the "No task list was active" sentence and NOTHING else —
///   no `## What's next` heading at all.
/// * The `- [>] …    ← current step` marker is separated by FOUR spaces.
#[must_use]
pub fn render_resume_md(doc: &ResumeDoc<'_>) -> String {
    let product = branding::PRODUCT_NAME;
    let mut lines: Vec<String> = vec![
        format!("# {product} \u{2014} resume checkpoint"),
        String::new(),
        format!("Session: {}", doc.session_id),
        format!("Written: {}", doc.written_iso),
        format!("Trigger: {}", doc.trigger.label()),
        format!("Ref with your in-progress files: {}", doc.ref_name),
        String::new(),
        "## To resume".to_string(),
        String::new(),
        format!("    lingxi --resume {}", doc.session_id),
        String::new(),
        format!("(or open {product} in this directory and run /resume)"),
        String::new(),
        "## Plan (from TodoWrite state)".to_string(),
        String::new(),
    ];

    if doc.todos.is_empty() {
        lines.push(
            "No task list was active; see transcript via the resume command above.".to_string(),
        );
    } else {
        // `r` — the first PENDING content; `n` — the first IN-PROGRESS
        // activeForm. Both are `??=`, i.e. first-wins, and both hold the
        // SANITISED text (the oracle assigns from the sanitised local).
        let mut first_pending: Option<String> = None;
        let mut first_in_progress: Option<String> = None;
        for todo in doc.todos {
            match todo.status {
                TodoState::Completed => {
                    lines.push(format!("- [x] {}", sanitize_todo_line(&todo.content)));
                }
                TodoState::InProgress => {
                    let active = sanitize_todo_line(&todo.active_form);
                    lines.push(format!("- [>] {active}    \u{2190} current step"));
                    if first_in_progress.is_none() {
                        first_in_progress = Some(active);
                    }
                }
                TodoState::Pending => {
                    let content = sanitize_todo_line(&todo.content);
                    lines.push(format!("- [ ] {content}"));
                    if first_pending.is_none() {
                        first_pending = Some(content);
                    }
                }
            }
        }
        lines.push(String::new());
        lines.push("## What's next".to_string());
        lines.push(String::new());
        lines.push(match (first_pending, first_in_progress) {
            (Some(pending), _) => pending,
            (None, Some(active)) => format!("finish: {active}"),
            (None, None) => "All tasks completed.".to_string(),
        });
    }

    lines.extend([
        String::new(),
        "---".to_string(),
        String::new(),
        "Don't want these changes? Resume this session (above), then run".to_string(),
        "`/rewind` to roll back the turn's tool edits (bash-made changes".to_string(),
        format!(
            "excluded). {} holds a full snapshot until this session's",
            doc.ref_name
        ),
        "next checkpoint, or for up to ~2 weeks.".to_string(),
        String::new(),
    ]);

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn todo(content: &str, status: TodoState, active_form: &str) -> TodoItem {
        TodoItem {
            id: String::new(),
            content: content.to_string(),
            status,
            active_form: active_form.to_string(),
        }
    }

    fn doc(todos: &[TodoItem], trigger: CheckpointTrigger) -> ResumeDoc<'_> {
        ResumeDoc {
            session_id: "abcd1234-5678-4abc-9def-000000000000",
            ref_name: "refs/lingxi/checkpoint-abcd1234",
            trigger,
            todos,
            written_iso: "2026-08-20T15:08:27.000Z",
        }
    }

    /// `RESUME_MD_REPO_PATH` is a literal because it has to be a `const`; this
    /// pins it to the one namespace source so a rebrand cannot leave it behind.
    #[test]
    fn resume_path_tracks_the_branding_dot_dir() {
        assert_eq!(
            RESUME_MD_REPO_PATH,
            format!("{}/RESUME.md", branding::DOT_DIR)
        );
    }

    #[test]
    fn ref_is_the_prefix_plus_eight_session_id_chars() {
        assert_eq!(
            checkpoint_ref("abcd1234-5678-4abc-9def-000000000000"),
            "refs/lingxi/checkpoint-abcd1234"
        );
        // Shorter than 8: `slice(0,8)` clamps, it does not pad or panic.
        assert_eq!(checkpoint_ref("ab"), "refs/lingxi/checkpoint-ab");
        assert_eq!(checkpoint_ref(""), "refs/lingxi/checkpoint-");
    }

    /// The empty-list document, whole. Byte-for-byte against `K4v` with the two
    /// documented rebrands — note there is NO `## What's next` section.
    #[test]
    fn empty_todo_list_document_is_byte_exact() {
        let rendered = render_resume_md(&doc(&[], CheckpointTrigger::NearLimit));
        assert_eq!(
            rendered,
            "# LingXi \u{2014} resume checkpoint\n\
             \n\
             Session: abcd1234-5678-4abc-9def-000000000000\n\
             Written: 2026-08-20T15:08:27.000Z\n\
             Trigger: near-limit\n\
             Ref with your in-progress files: refs/lingxi/checkpoint-abcd1234\n\
             \n\
             ## To resume\n\
             \n    lingxi --resume abcd1234-5678-4abc-9def-000000000000\n\
             \n\
             (or open LingXi in this directory and run /resume)\n\
             \n\
             ## Plan (from TodoWrite state)\n\
             \n\
             No task list was active; see transcript via the resume command above.\n\
             \n\
             ---\n\
             \n\
             Don't want these changes? Resume this session (above), then run\n\
             `/rewind` to roll back the turn's tool edits (bash-made changes\n\
             excluded). refs/lingxi/checkpoint-abcd1234 holds a full snapshot until this session's\n\
             next checkpoint, or for up to ~2 weeks.\n"
        );
        assert!(
            !rendered.contains("What's next"),
            "the heading lives inside the non-empty branch only"
        );
    }

    /// The plan block: one line per todo in list order, the in-progress marker,
    /// and "what's next" = the first PENDING content (a pending item outranks
    /// the in-progress one even when it comes after it).
    #[test]
    fn plan_block_renders_every_status_and_prefers_the_first_pending() {
        let todos = [
            todo("Read the audit", TodoState::Completed, "Reading the audit"),
            todo(
                "Port the renderer",
                TodoState::InProgress,
                "Porting the renderer",
            ),
            todo("Wire the trigger", TodoState::Pending, ""),
            todo("Write the report", TodoState::Pending, ""),
        ];
        let rendered = render_resume_md(&doc(&todos, CheckpointTrigger::RateLimited));
        assert!(rendered.contains("Trigger: rate-limited\n"));
        assert!(
            rendered.contains(
                "## Plan (from TodoWrite state)\n\
                 \n\
                 - [x] Read the audit\n\
                 - [>] Porting the renderer    \u{2190} current step\n\
                 - [ ] Wire the trigger\n\
                 - [ ] Write the report\n\
                 \n\
                 ## What's next\n\
                 \n\
                 Wire the trigger\n"
            ),
            "{rendered}"
        );
    }

    /// No pending item ⇒ `finish: <first in-progress activeForm>`; nothing left
    /// at all ⇒ `All tasks completed.`
    #[test]
    fn whats_next_falls_back_to_the_in_progress_step_then_to_all_completed() {
        let in_progress = [
            todo("Done", TodoState::Completed, "Doing"),
            todo("Porting", TodoState::InProgress, "Porting the executor"),
        ];
        let rendered = render_resume_md(&doc(&in_progress, CheckpointTrigger::NearLimit));
        assert!(
            rendered.contains("\n\n## What's next\n\nfinish: Porting the executor\n"),
            "{rendered}"
        );

        let all_done = [todo("Done", TodoState::Completed, "Doing")];
        let rendered = render_resume_md(&doc(&all_done, CheckpointTrigger::NearLimit));
        assert!(
            rendered.contains("\n\n## What's next\n\nAll tasks completed.\n"),
            "{rendered}"
        );
    }

    /// A todo cannot forge markdown structure: every newline / control /
    /// bidi-override RUN collapses to ONE space, so the item stays one line.
    #[test]
    fn sanitiser_collapses_runs_and_keeps_a_todo_on_one_line() {
        assert_eq!(
            sanitize_todo_line("line one\n\n- [x] forged\ttail"),
            "line one - [x] forged tail"
        );
        // U+202E RIGHT-TO-LEFT OVERRIDE and U+200B ZERO WIDTH SPACE are in the
        // class; U+2066..U+2069 (the isolates) too.
        assert_eq!(
            sanitize_todo_line("a\u{202e}\u{200b}\u{2069}b"),
            "a b",
            "an adjacent run is one space, not three"
        );
        // Ordinary text is untouched, including non-ASCII.
        assert_eq!(sanitize_todo_line("端到端 · ok"), "端到端 · ok");

        let todos = [todo("x\ny", TodoState::Pending, "")];
        let rendered = render_resume_md(&doc(&todos, CheckpointTrigger::NearLimit));
        assert!(rendered.contains("- [ ] x y\n"), "{rendered}");
    }

    /// 500 UTF-16 units, then `…` — and the cap is applied AFTER sanitising, so
    /// a run that collapses under the limit is not truncated.
    #[test]
    fn sanitiser_truncates_at_500_utf16_units_with_an_ellipsis() {
        let exactly = "a".repeat(TODO_LINE_MAX_UTF16);
        assert_eq!(sanitize_todo_line(&exactly), exactly, "the cap is exclusive");

        let over = "a".repeat(TODO_LINE_MAX_UTF16 + 1);
        let cut = sanitize_todo_line(&over);
        assert_eq!(cut.chars().count(), TODO_LINE_MAX_UTF16 + 1);
        assert!(cut.ends_with('\u{2026}'));
        assert_eq!(&cut[..TODO_LINE_MAX_UTF16], exactly);

        // A non-BMP char is TWO UTF-16 units: 250 of them exactly fill the cap.
        let emoji = "\u{1f600}".repeat(TODO_LINE_MAX_UTF16 / 2);
        assert_eq!(sanitize_todo_line(&emoji), emoji);
        let emoji_over = format!("{emoji}\u{1f600}");
        let cut = sanitize_todo_line(&emoji_over);
        assert_eq!(
            cut,
            format!("{emoji}\u{2026}"),
            "a surrogate pair is never split in half"
        );
    }

    #[test]
    fn trigger_and_skip_reason_wire_spellings() {
        assert_eq!(CheckpointTrigger::NearLimit.as_str(), "near_limit");
        assert_eq!(CheckpointTrigger::NearLimit.label(), "near-limit");
        assert_eq!(CheckpointTrigger::RateLimited.as_str(), "rate_limited");
        assert_eq!(CheckpointTrigger::RateLimited.label(), "rate-limited");

        // All 14, in the order `q4v` tests them.
        let reasons = [
            CheckpointSkipReason::NonInteractive,
            CheckpointSkipReason::RemoteWorkspace,
            CheckpointSkipReason::Policy,
            CheckpointSkipReason::NotGit,
            CheckpointSkipReason::BareRepo,
            CheckpointSkipReason::GitRootUncontained,
            CheckpointSkipReason::GitDirUncontained,
            CheckpointSkipReason::SparseCheckout,
            CheckpointSkipReason::ContentFilters,
            CheckpointSkipReason::SequencerInProgress,
            CheckpointSkipReason::NoHead,
            CheckpointSkipReason::TooLarge,
            CheckpointSkipReason::ResumeWriteRefused,
            CheckpointSkipReason::GitError,
        ];
        let spellings: Vec<&str> = reasons.iter().map(|r| r.as_str()).collect();
        assert_eq!(
            spellings,
            vec![
                "non_interactive",
                "remote_workspace",
                "policy",
                "not_git",
                "bare_repo",
                "gitroot_uncontained",
                "gitdir_uncontained",
                "sparse_checkout",
                "content_filters",
                "sequencer_in_progress",
                "no_head",
                "too_large",
                "resume_write_refused",
                "git_error",
            ]
        );
    }
}
