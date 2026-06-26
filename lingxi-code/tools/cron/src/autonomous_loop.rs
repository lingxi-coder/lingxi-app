//! Autonomous-loop subsystem — 1:1 port of the 2.1.191 binary's `T3e` module
//! (`mt(T3e,{resolveLoopFileFire,resolveLoopDefaultFire,resolveAutonomousLoopFire,
//! resetAutonomousLoopDelivered,readLoopFile,…})`, cc_all.txt:504950).
//!
//! The `/loop` dynamic / autonomous modes pass a *sentinel* string back through
//! `ScheduleWakeup`'s `prompt` arg (or a CronCreate prompt). At fire time the
//! runtime resolves the sentinel → the actual tick prompt:
//!   - first delivery: the full autonomous-loop PREAMBLE + the tick instructions
//!   - subsequent deliveries: a short reminder tick only ("the long instructions
//!     stay in the cached message-prefix").
//!
//! Sentinels (binary literals):
//!   - `<<autonomous-loop>>`         (`Wst`) — CronCreate-based autonomous loop
//!   - `<<autonomous-loop-dynamic>>` (`Jke`) — ScheduleWakeup self-paced loop
//!   - `<<loop.md>>`                 (`rKi`) — CronCreate loop.md-tasks loop
//!   - `<<loop.md-dynamic>>`         (`lFt`) — ScheduleWakeup loop.md-tasks loop
//!
//! The first-vs-subsequent delivery state (`iFt` = preamble-delivered,
//! `Gst` = last-delivered loop.md content / preamble-sentinel) is module-level
//! mutable global in the binary, shared across every fire. The port mirrors that
//! with a process-global [`Mutex`]; `resolve_wakeup_prompt` (called at fire time
//! by the bridge `MsgQueueWakeupScheduler`) reads/updates it so the second fire
//! drops the long preamble exactly like the binary.
//!
//! FEATURE FLAGS: the binary gates the *resolution* on `isLoopDefaultPromptEnabled`
//! = `tengu_kairos_loop_prompt` (default **false**), the *preamble variant* on
//! `isLoopPersistentPreambleEnabled` = env `CLAUDE_CODE_LOOP_PERSISTENT` ||
//! `tengu_kairos_loop_persistent` (default false), and the `PushNotification`
//! addendum on `Yke()` = `tengu_kairos_push_notifications` && `agentPushNotifEnabled`
//! (default false). The port's `features` crate has no `tengu_kairos_loop_*`
//! keys, so each flag defaults to the binary's shipped default; the env var
//! `CLAUDE_CODE_LOOP_PERSISTENT` IS honored (matching `rt(process.env.…)`).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

// ── Sentinels (binary string-table) ──────────────────────────────────────────

/// `Wst` (cc_all.txt:504931) — CronCreate-based autonomous loop sentinel.
pub const AUTONOMOUS_LOOP_SENTINEL: &str = "<<autonomous-loop>>";
/// `Jke` (cc_all.txt:504931) — ScheduleWakeup self-paced autonomous sentinel.
pub const AUTONOMOUS_LOOP_DYNAMIC_SENTINEL: &str = "<<autonomous-loop-dynamic>>";
/// `rKi` (cc_all.txt:504966) — CronCreate loop.md-tasks sentinel.
pub const LOOP_FILE_SENTINEL: &str = "<<loop.md>>";
/// `lFt` (cc_all.txt:504966) — ScheduleWakeup loop.md-tasks sentinel.
pub const LOOP_FILE_DYNAMIC_SENTINEL: &str = "<<loop.md-dynamic>>";

/// `ZVi` (cc_all.txt:504966) — the placeholder stored in `Gst` once the preamble
/// has been delivered through the loop.md-default path (so a later loop.md-absent
/// fire knows the preamble already shipped).
const PREAMBLE_SENTINEL: &str = "__autonomous_preamble__";

/// `zIn` (cc_all.txt:504966) — loop.md truncation budget in bytes.
const LOOP_FILE_MAX_BYTES: usize = 25_000;

/// `Kh` (cc_all.txt:504932) — the `ScheduleWakeup` tool name, interpolated into
/// every tick prompt.
const SCHEDULE_WAKEUP: &str = "ScheduleWakeup";
/// `IA` (cc_all.txt:504932) — the `Monitor` tool name.
const MONITOR: &str = "Monitor";
/// `AI` (cc_all.txt:504945) — the `TaskList` tool name.
const TASK_LIST: &str = "TaskList";
/// `eP` (cc_all.txt:504945) — the `TaskStop` tool name.
const TASK_STOP: &str = "TaskStop";
/// `Z8` (cc_all.txt:504927) — the `PushNotification` tool name.
const PUSH_NOTIFICATION: &str = "PushNotification";

// ── Feature-flag gates (binary defaults; see module docs) ────────────────────

/// `YIn` / `isLoopPersistentPreambleEnabled` (cc_all.txt:504950):
/// `rt(process.env.CLAUDE_CODE_LOOP_PERSISTENT) || nt("tengu_kairos_loop_persistent",false)`.
// PARITY: env `CLAUDE_CODE_LOOP_PERSISTENT` is honored; the `tengu_kairos_loop_persistent`
// flag has no port backend so it defaults to the binary's shipped `false`.
#[must_use]
pub fn is_loop_persistent_preamble_enabled() -> bool {
    env_truthy("CLAUDE_CODE_LOOP_PERSISTENT")
}

/// `fJr` / `isLoopDefaultPromptEnabled` (cc_all.txt:504952):
/// `nt("tengu_kairos_loop_prompt",false)`. Gates whether the autonomous/loop.md
/// sentinels resolve at all (else `J4d` passes them through verbatim).
// PARITY: `tengu_kairos_loop_prompt` has no port backend → it DEFAULTS to the
// binary's shipped `false`, so the sentinels pass through verbatim exactly like
// the shipped binary (where `J4d(e)=nKi(e)??sKi(e)??e` returns `e` unchanged when
// `fJr()` is false). This deliberately matches the binary default rather than the
// prior synthesized always-expand behavior, which WAS anti-parity.
//
// PARITY (port extension): because the binary gates this on a SERVER-side feature
// flag (which Anthropic flips on to ship the feature) and the port has no flag
// backend, the env var `CLAUDE_CODE_LOOP_PROMPT` stands in for that server flip —
// mirroring the established `CLAUDE_CODE_LOOP_PERSISTENT` env override. With it
// unset the port is byte-identical to the shipped binary (sentinels inert); set
// it (e.g. on a host that wants the live ScheduleWakeup self-pace round-trip) and
// the resolver expands sentinels exactly as the flag-on binary would.
// PARITY-TODO: replace the env override with the real `tengu_kairos_loop_prompt`
// flag once the `features` crate exposes it.
#[must_use]
pub fn is_loop_default_prompt_enabled() -> bool {
    env_truthy("CLAUDE_CODE_LOOP_PROMPT")
}

/// `q_e` (cc_all.txt:521920 dispatch): `nt("tengu_kairos_loop_dynamic",false)`.
/// Selects the DYNAMIC-pacing builders (`hZm` usage / `gZm` prompt-builder, and
/// `a(loopFile,true)` for the no-prompt autonomous default) over the cron variants.
// PARITY: `tengu_kairos_loop_dynamic` has no port backend → it DEFAULTS to the
// binary's shipped `false`. As with [`is_loop_default_prompt_enabled`] the env
// var `CLAUDE_CODE_LOOP_DYNAMIC` stands in for the server flag flip (mirroring
// the `CLAUDE_CODE_LOOP_PROMPT` / `CLAUDE_CODE_LOOP_PERSISTENT` overrides); unset
// it and the port matches the shipped binary (cron variants only).
// PARITY-TODO: replace the env override with the real `tengu_kairos_loop_dynamic`
// flag once the `features` crate exposes it.
#[must_use]
pub fn is_loop_dynamic_enabled() -> bool {
    env_truthy("CLAUDE_CODE_LOOP_DYNAMIC")
}

/// `Yke` (cc_all.txt:504927): `Rle() && agentPushNotifEnabled`. Gates the
/// `PushNotification` addendum appended to tick prompts.
// PARITY: both `tengu_kairos_push_notifications` and the `agentPushNotifEnabled`
// setting have no port backend → binary default `false` ⇒ `aFt()` returns "".
#[must_use]
fn is_push_notif_enabled() -> bool {
    false
}

fn env_truthy(key: &str) -> bool {
    // PARITY: binary `rt(x)` truthiness — present & not one of the falsey strings.
    match std::env::var(key) {
        Ok(v) => {
            let t = v.trim();
            !(t.is_empty() || t == "0" || t.eq_ignore_ascii_case("false"))
        }
        Err(_) => false,
    }
}

// ── Preambles (binary `aJr` / `VVi`) ─────────────────────────────────────────

/// `aJr` (cc_all.txt:504904-504914) — the default (non-persistent) autonomous-
/// loop preamble; `j4d` = `AUTONOMOUS_LOOP_PREAMBLE` is `aJr`. Verbatim incl.
/// trailing newline (the binary template literal ends with one).
// PARITY: binary aJr (cc_all.txt:504904).
const PREAMBLE_DEFAULT: &str = "# Autonomous loop check
You're being invoked on a timer while the user is away or occupied. The point is to keep work moving forward without the user driving every step — finishing things they started, maintaining PRs they're building, catching problems before they come back to find them. You're a steward, not an initiator. The user set you loose on their work, and the value you provide comes from reliably advancing things they've already set in motion, not from finding new things to do.
The key tension to navigate: the user trusts you enough to run autonomously, but that trust is easily lost. Acting on what the conversation already established is safe and valuable. Inventing new work or making irreversible changes without clear authorization erodes trust fast. When you're unsure whether something falls into \"continuing established work\" or \"inventing new work,\" lean toward the former only when the transcript provides clear evidence the user wanted it done. If you find yourself reaching for justifications about why a push is probably fine, that's a signal to wait.
## What to act on
The current conversation is your highest-signal source — re-read the transcript above, since everything there is something the user was actively engaged with. The strongest signal is an in-progress PR you've been building together: review comments to address and resolve, failing CI checks to diagnose (and re-enqueue if they're flakes), merge conflicts to fix. The goal is to get the PR into a state where it's ready to merge pending only human review — the user shouldn't come back to find a PR blocked on things you could have handled. After that, look for unfinished implementation where the last exchange left something half-done, and explicit \"I'll also...\" or \"next I'll...\" commitments the conversation made and didn't honor. Weaker but still real: dangling questions you could now answer, verification steps that were skipped, edge cases that were mentioned but not handled, and natural continuations that don't require new decisions.
If you find anything in this category, act on it — actually do the work, don't describe what could be done. Run the tests, don't say \"you could run the tests.\" The whole point of autonomous operation is that work gets done while the user is away.
When the conversation transcript has nothing left, the current branch's pull/merge request on the user's SCM is the next-best place to look. This is maintenance work — valuable, but lower priority than continuing the user's active work. Find the PR/MR for the current branch via the SCM's CLI, then check three things: CI status, unresolved review threads, and whether the branch has fallen behind the base. For failing CI, pull the failing job's logs and diagnose before acting — flaky-shaped failures (timeout, runner died, transient network) can be re-enqueued; real failures need a reproduction and a minimal fix. For unresolved review threads, fetch the comment, address the feedback, push, and resolve the thread via, for example, the GitHub GraphQL `resolveReviewThread` mutation (or the equivalent for whichever SCM the project uses). Before pushing anything, check whether someone else has pushed to the branch while you were working — if so, rebase (don't merge) to keep history clean.
When CI is green, threads are clear, and there's idle time, sweeping the branch for issues is a good use of that time — bug-hunt or simplification passes catch problems before reviewers do, saving everyone a round-trip.
If everything is genuinely quiet — no conversation work, no PR maintenance — say so in one sentence and stop. No summary of what you checked, no list of what you might do later. The user will see your message in the transcript when they come back; three consecutive \"nothing to do\" results means you should scale back to a quick CI check and stop, not narrate.
## Repeated invocations
If you see earlier autonomous checks in this conversation, adjust your scope accordingly. If a previous check left a question the user hasn't answered, the cost of acting depends on reversibility: for reversible actions (local edits, running tests), make your best call and proceed; for irreversible ones (pushing, deleting, sending), keep waiting — the cost of acting wrongly on something irreversible is much higher than the cost of waiting one more cycle. If three or more consecutive checks have found nothing actionable, things are quiet — do one quick CI/threads check and stop in a single line. Repeated \"nothing to do\" messages clutter the transcript and waste the user's attention when they come back to review.
Read and analyze freely — understanding the state of things has no blast radius. Make edits and run tests when you're confident they continue established work. Commit and push only when you're clearly continuing something the user authorized, or when the work pattern makes the intent obvious — like fixing CI on a PR you've been building together.
";

/// `VVi` (cc_all.txt:504916-504926) — the persistent-preamble variant
/// (`getAutonomousLoopPreamble` returns this when `isLoopPersistentPreambleEnabled`).
/// Verbatim incl. trailing newline.
// PARITY: binary VVi (cc_all.txt:504916).
const PREAMBLE_PERSISTENT: &str = "# Autonomous loop check
You're being invoked on a timer while the user is away or occupied. The point is to keep work moving forward without the user driving every step — finishing things they started, maintaining PRs they're building, catching problems before they come back to find them, and following through on the *spirit* of the task they gave you, not just its literal scope. The user set you loose on their work, and the value you provide comes from reliably advancing things they've already set in motion.
The key tension to navigate: the user trusts you enough to run autonomously, but that trust is easily lost. Acting on what the conversation already established is safe and valuable. For irreversible actions (push, delete, send), require clear authorization in the transcript or use a reversible alternative (a draft, a local commit, a queued message). For reversible actions (edits, tests, drafts, exploration), bias toward acting — the cost of an unneeded local edit is near zero, and the cost of a stalled loop is high. When you're unsure whether something falls into \"continuing established work\" or \"inventing new work,\" lean toward continuing whenever the transcript gives you any reasonable thread to pull on.
## What to act on
The current conversation is your highest-signal source — re-read the transcript above, since everything there is something the user was actively engaged with. The strongest signal is an in-progress PR you've been building together: review comments to address and resolve, failing CI checks to diagnose (and re-enqueue if they're flakes), merge conflicts to fix. The goal is to get the PR into a state where it's ready to merge pending only human review — the user shouldn't come back to find a PR blocked on things you could have handled. After that, look for unfinished implementation where the last exchange left something half-done, and explicit \"I'll also...\" or \"next I'll...\" commitments the conversation made and didn't honor. Weaker but still real: dangling questions you could now answer, verification steps that were skipped, edge cases that were mentioned but not handled, and natural continuations that don't require new decisions.
If you find anything in this category, act on it — actually do the work, don't describe what could be done. Run the tests, don't say \"you could run the tests.\" The whole point of autonomous operation is that work gets done while the user is away.
When the conversation transcript has nothing left, the current branch's pull/merge request on the user's SCM is the next-best place to look. This is maintenance work — valuable, but lower priority than continuing the user's active work. Find the PR/MR for the current branch via the SCM's CLI, then check three things: CI status, unresolved review threads, and whether the branch has fallen behind the base. For failing CI, pull the failing job's logs and diagnose before acting — flaky-shaped failures (timeout, runner died, transient network) can be re-enqueued; real failures need a reproduction and a minimal fix. For unresolved review threads, fetch the comment, address the feedback, push, and resolve the thread via, for example, the GitHub GraphQL `resolveReviewThread` mutation (or the equivalent for whichever SCM the project uses). Before pushing anything, check whether someone else has pushed to the branch while you were working — if so, rebase (don't merge) to keep history clean.
When CI is green, threads are clear, and there's idle time, sweeping the branch for issues is a good use of that time — bug-hunt or simplification passes catch problems before reviewers do, saving everyone a round-trip.
If everything is genuinely quiet — no conversation work, no PR maintenance — say so in one sentence and keep the loop alive. Before stopping, broaden once: re-read the original task framing, check whether earlier ticks deferred anything (\"I'll wait for X\"), and look at sibling PRs/branches the user owns. Persistence is the point of autonomous mode. Only stop if the original task is provably complete or the user said to stop. (Pacing — how long to wait before the next tick — is handled by the per-mode reminder appended to this preamble; don't try to manage delay from here.)
## Repeated invocations
If you see earlier autonomous checks in this conversation, adjust your scope accordingly. If a previous check left a question the user hasn't answered, the cost of acting depends on reversibility: for reversible actions (local edits, running tests), make your best call and proceed; for irreversible ones (pushing, deleting, sending), keep waiting — the cost of acting wrongly on something irreversible is much higher than the cost of waiting one more cycle. If three or more consecutive checks have found nothing actionable, broaden scope once before considering stopping — re-read the original task, check sibling work, look for verification or polish steps that were skipped. A loop that quits the moment work goes quiet is less useful than one that waits.
Read and analyze freely — understanding the state of things has no blast radius. Make edits and run tests when you're confident they continue established work. Commit and push only when you're clearly continuing something the user authorized, or when the work pattern makes the intent obvious — like fixing CI on a PR you've been building together.
";

/// `AUTONOMOUS_LOOP_PREAMBLE` (`j4d`, cc_all.txt:504950) = the default preamble.
pub const AUTONOMOUS_LOOP_PREAMBLE: &str = PREAMBLE_DEFAULT;

/// `dJr` / `getAutonomousLoopPreamble` (cc_all.txt:504950):
/// `isLoopPersistentPreambleEnabled() ? VVi : aJr`.
#[must_use]
pub fn get_autonomous_loop_preamble() -> &'static str {
    if is_loop_persistent_preamble_enabled() {
        PREAMBLE_PERSISTENT
    } else {
        PREAMBLE_DEFAULT
    }
}

/// `pJr` / `logAutonomousLoopActivation` (cc_all.txt:504950):
/// `W("tengu_kairos_loop_persistent_activated",{variant:YIn()})`. Emitted when
/// the autonomous-loop default is activated — by the `/loop` command's no-prompt
/// builder (binary `a(c,u)` when loop.md is absent) and by the fire-time
/// resolvers `nKi` (every autonomous fire) / `sKi` (loop.md-absent fire).
// PARITY: binary pJr (cc_all.txt:504950).
pub fn log_autonomous_loop_activation() {
    telemetry::emit_loop_persistent_activated(is_loop_persistent_preamble_enabled());
}

// ── PushNotification addendum (binary `aFt`) ─────────────────────────────────

/// `aFt(e)` (cc_all.txt:504950) — the `PushNotification` block appended to tick
/// prompts. Returns "" when `Yke()` is off (the port's default). `cron` selects
/// the "you're ending the loop" phrasing (cron variant) vs the "third straight
/// tick…" phrasing (dynamic variant, when persistent preamble is on).
// PARITY: binary aFt (cc_all.txt:504950-504951).
fn push_notif_addendum(cron: bool) -> String {
    if !is_push_notif_enabled() {
        return String::new();
    }
    // PARITY: `let n=!e&&YIn()?…:…` — `e`=cron flag, `YIn()`=persistent.
    let n = if !cron && is_loop_persistent_preamble_enabled() {
        "newly blocked on a decision you won't make alone, you're ending the loop"
    } else {
        "newly blocked on a decision you won't make alone, third straight tick with nothing to do, you're ending the loop"
    };
    format!(
        "\nUse {PUSH_NOTIFICATION} when the loop can't move further without the user, or when something landed that they'd want to act on now: {n}, or a major update arrived (CI went red, a review changes the plan). Progress you made yourself isn't a trigger — the transcript covers that. One ping per state, not per tick."
    )
}

/// `mJr` (cc_all.txt:504966) — the Monitor/keepalive fallback addendum appended
/// to every DYNAMIC tick prompt. Interpolates Monitor/TaskList/TaskStop names.
// PARITY: binary mJr (cc_all.txt:504966).
fn monitor_addendum() -> String {
    format!(
        "\nIf a {MONITOR} is armed (check {TASK_LIST}), keep `delaySeconds` at 1200–1800s — the {MONITOR} is the wake signal and this is only the fallback heartbeat. If you were woken by a `<task-notification>`, handle the event before rescheduling. To stop the loop, also {TASK_STOP} the monitor (use {TASK_LIST} to find its task ID if no longer in context)."
    )
}

// ── Tick-prompt builders (binary `tKi`/`W4d`/`G4d`/`V4d`/`K4d`) ───────────────

/// `tKi` (cc_all.txt:504951) — autonomous loop tick (cron mode).
fn tick_autonomous_cron() -> String {
    format!(
        "# Autonomous loop tick\nRun the autonomous check using the loop instructions established earlier in this conversation. If you cannot find them, treat this as a no-op tick. The recurring cron will fire the next tick automatically — do not call {SCHEDULE_WAKEUP} from this tick.{addendum}",
        addendum = push_notif_addendum(false),
    )
}

/// `W4d` (cc_all.txt:504952) — autonomous loop tick (dynamic pacing).
fn tick_autonomous_dynamic() -> String {
    format!(
        "# Autonomous loop tick (dynamic pacing)\nRun the autonomous check using the loop instructions established earlier in this conversation. If you cannot find them, treat this as a no-op tick.\nYou scheduled this tick via the {SCHEDULE_WAKEUP} tool (not a recurring cron). To keep the loop alive, call {SCHEDULE_WAKEUP} again at the end of this turn with `prompt` set to the literal sentinel `{AUTONOMOUS_LOOP_DYNAMIC_SENTINEL}` — otherwise the loop ends after this tick.{monitor}{push}",
        monitor = monitor_addendum(),
        push = push_notif_addendum(false),
    )
}

/// `G4d` (cc_all.txt:504952) — loop.md tasks tick (cron mode).
fn tick_loopfile_cron() -> String {
    format!(
        "# /loop tick — loop.md tasks\nWork the tasks from the loop.md contents established earlier in this conversation. If you cannot find them, treat this as a no-op tick. The recurring cron will fire the next tick automatically — do not call {SCHEDULE_WAKEUP} from this tick.{addendum}",
        addendum = push_notif_addendum(true),
    )
}

/// `V4d` (cc_all.txt:504952) — loop.md tasks tick (dynamic pacing).
fn tick_loopfile_dynamic() -> String {
    format!(
        "# /loop tick — loop.md tasks (dynamic pacing)\nWork the tasks from the loop.md contents established earlier in this conversation. If you cannot find them, treat this as a no-op tick.\nYou scheduled this tick via the {SCHEDULE_WAKEUP} tool (not a recurring cron). To keep the loop alive, call {SCHEDULE_WAKEUP} again at the end of this turn with `prompt` set to the literal sentinel `{LOOP_FILE_DYNAMIC_SENTINEL}` — otherwise the loop ends after this tick.{monitor}{push}",
        monitor = monitor_addendum(),
        push = push_notif_addendum(true),
    )
}

/// `K4d` (cc_all.txt:504952) — loop.md ABSENT tick (dynamic pacing).
fn tick_loopfile_absent_dynamic() -> String {
    format!(
        "# /loop tick — loop.md absent (dynamic pacing)\nloop.md is not currently present. Run the autonomous check using the loop instructions established earlier in this conversation.\nYou scheduled this tick via the {SCHEDULE_WAKEUP} tool (not a recurring cron). To keep the loop alive — and to pick up loop.md if it is recreated — call {SCHEDULE_WAKEUP} again at the end of this turn with `prompt` set to the literal sentinel `{LOOP_FILE_DYNAMIC_SENTINEL}` — otherwise the loop ends after this tick.{monitor}{push}",
        monitor = monitor_addendum(),
        push = push_notif_addendum(false),
    )
}

// ── loop.md reader (binary `oKi` + truncation `z4d`) ──────────────────────────

/// `z4d(e)` (cc_all.txt:504965) — truncate loop.md content to `zIn` bytes with a
/// warning footer, preferring to cut at the last newline before the budget.
// PARITY: binary z4d (cc_all.txt:504965).
fn truncate_loop_file(content: &str) -> String {
    if content.len() <= LOOP_FILE_MAX_BYTES {
        return content.to_string();
    }
    // PARITY: `let t=e.lastIndexOf("\n",zIn)` — last newline at/before the budget.
    // The binary's JS `e.slice` / `lastIndexOf` operate on UTF-16 code units and
    // never panic at a code-unit boundary. Rust string slicing panics on a non-
    // char-boundary index, so clamp the budget DOWN to the nearest char boundary
    // (`<= LOOP_FILE_MAX_BYTES`) before slicing — a multibyte char straddling
    // byte 25000 (CJK / emoji / accented text, common in a real loop.md) would
    // otherwise panic at fire time. This is behavior-preserving vs the binary
    // (the warning footer + budget value are unchanged).
    let mut budget = LOOP_FILE_MAX_BYTES.min(content.len());
    while budget > 0 && !content.is_char_boundary(budget) {
        budget -= 1;
    }
    let cut = content[..budget]
        .rfind('\n')
        .filter(|&i| i > 0)
        .unwrap_or(budget);
    let head = &content[..cut];
    format!(
        "{head}\n> WARNING: loop.md was truncated to {LOOP_FILE_MAX_BYTES} bytes. Keep the task list concise."
    )
}

/// A located loop.md file: its path and (truncated) contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopFile {
    /// The file the contents were read from.
    pub path: PathBuf,
    /// Trimmed + truncated contents.
    pub content: String,
}

/// `oKi` / `readLoopFile` (cc_all.txt:504966): reads `<cwd>/.claude/loop.md` then
/// `<cwd>/loop.md`, trims, skips empty, truncates to `zIn` bytes. Returns the
/// first non-empty match (path + content) or `None`.
// PARITY: binary oKi (cc_all.txt:504966). The binary uses `dc()` (project root)
// for `.claude/loop.md` and `Zn()` (cwd) for `loop.md`; the port reads both
// relative to the supplied `cwd` (the bridge passes the session cwd; in practice
// `dc()==Zn()` for a single-project session).
#[must_use]
pub fn read_loop_file(cwd: &Path) -> Option<LoopFile> {
    let candidates = [cwd.join(".claude").join("loop.md"), cwd.join("loop.md")];
    for path in candidates {
        let raw = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            // PARITY: binary `if(zo(o)||dn(o)==="EISDIR")continue;throw o` — skip
            // not-found / is-a-directory; the port treats any read error as skip
            // (no panic at fire time).
            Err(_) => continue,
        };
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        return Some(LoopFile {
            path,
            content: truncate_loop_file(trimmed),
        });
    }
    None
}

// ── Sentinel predicates (binary `hJr`/`gJr`/`Y4d`) ───────────────────────────

/// `hJr` / `isAutonomousLoopSentinel` (cc_all.txt:504952).
#[must_use]
pub fn is_autonomous_loop_sentinel(s: &str) -> bool {
    s == AUTONOMOUS_LOOP_SENTINEL || s == AUTONOMOUS_LOOP_DYNAMIC_SENTINEL
}

/// `gJr` / `isLoopFileSentinel` (cc_all.txt:504966).
#[must_use]
pub fn is_loop_file_sentinel(s: &str) -> bool {
    s == LOOP_FILE_SENTINEL || s == LOOP_FILE_DYNAMIC_SENTINEL
}

/// `Y4d` / `isLoopDefaultSentinel` (cc_all.txt:504966): any of the four.
#[must_use]
pub fn is_loop_default_sentinel(s: &str) -> bool {
    is_autonomous_loop_sentinel(s) || is_loop_file_sentinel(s)
}

// ── First-vs-subsequent delivery state (binary `iFt` / `Gst`) ─────────────────

/// Module-level delivery state, mirroring the binary's `iFt`/`Gst` globals
/// (cc_all.txt:504966 `var …,iFt=!1,Gst=null,…`). `preamble_delivered` = `iFt`;
/// `last_content` = `Gst` (the last loop.md content delivered, or the
/// `PREAMBLE_SENTINEL` placeholder once the preamble shipped via the default path).
#[derive(Default)]
struct DeliveryState {
    /// `iFt` — true once the full preamble has been delivered on a prior fire.
    preamble_delivered: bool,
    /// `Gst` — last delivered loop.md content (or the preamble placeholder).
    last_content: Option<String>,
}

static DELIVERY: Mutex<DeliveryState> = Mutex::new(DeliveryState {
    preamble_delivered: false,
    last_content: None,
});

/// `X4d` / `resetAutonomousLoopDelivered` (cc_all.txt:504966): clears the
/// first-delivery state so the next fire re-emits the full preamble. Called on a
/// fresh loop / user-abort.
///
// PARITY-TODO: in the binary, `X4d` is invoked by the user-abort path (`XIn`)
// and on fresh-loop start so the first-vs-subsequent delivery state is correct
// across distinct `/loop` sessions in one process. In the port this fn is NOT
// yet wired to a production loop-lifecycle event (only a test calls it). With
// the resolver gate OFF (`tengu_kairos_loop_prompt` default false, i.e.
// `CLAUDE_CODE_LOOP_PROMPT` unset) the `DELIVERY` state is never mutated, so
// this is inert and byte-matches the shipped binary. But under the documented
// `CLAUDE_CODE_LOOP_PROMPT=1` override a second `/loop` session in the same
// process would skip the full preamble (the first session left
// `preamble_delivered=true`). Wire this to the loop-end / user-abort and
// fresh-loop-start events once those seams exist; until then a process restart
// is the only reset.
pub fn reset_autonomous_loop_delivered() {
    let mut st = DELIVERY.lock().unwrap();
    st.preamble_delivered = false;
    st.last_content = None;
}

/// Process-wide serialization lock for tests that mutate the shared `DELIVERY`
/// state and/or the `CLAUDE_CODE_LOOP_*` env vars. Shared across this crate's
/// test modules (e.g. `wakeup::tests::sentinel_resolution`) so resolution tests
/// don't race each other over the globals.
#[cfg(test)]
pub(crate) static TEST_SERIAL: Mutex<()> = Mutex::new(());

// ── Fire resolvers (binary `nKi` / `sKi` / `J4d`) ────────────────────────────

/// `nKi` / `resolveAutonomousLoopFire` (cc_all.txt:504952): resolves an
/// autonomous sentinel → the tick prompt, prefixing the full preamble on first
/// delivery only. Returns `None` for a non-autonomous sentinel or when the gate
/// is off (passthrough handled by [`resolve_loop_default_fire`]).
#[must_use]
pub fn resolve_autonomous_loop_fire(sentinel: &str) -> Option<String> {
    if !is_autonomous_loop_sentinel(sentinel) {
        return None;
    }
    if !is_loop_default_prompt_enabled() {
        return None;
    }
    // PARITY: `nKi` calls `pJr()` on EVERY autonomous fire, right after the two
    // gate returns and before computing the tick (cc_all.txt:504952).
    log_autonomous_loop_activation();
    let tick = if sentinel == AUTONOMOUS_LOOP_DYNAMIC_SENTINEL {
        tick_autonomous_dynamic()
    } else {
        tick_autonomous_cron()
    };
    let mut st = DELIVERY.lock().unwrap();
    // PARITY: `if(iFt||Gst!==null)return t;return iFt=!0,`${dJr()}\n${t}``.
    if st.preamble_delivered || st.last_content.is_some() {
        return Some(tick);
    }
    st.preamble_delivered = true;
    Some(format!("{}\n{}", get_autonomous_loop_preamble(), tick))
}

/// `sKi` / `resolveLoopFileFire` (cc_all.txt:504966): resolves a loop.md sentinel
/// → the tick prompt. On first delivery (or whenever loop.md content changed
/// since last fire) it inlines the full loop.md contents; on unchanged fires it
/// returns the short reminder tick only. When loop.md is absent it falls back to
/// the autonomous preamble + absent-tick (first delivery) or the autonomous cron
/// tick (subsequent). `cwd` locates loop.md (binary uses process cwd).
#[must_use]
pub fn resolve_loop_file_fire(sentinel: &str, cwd: &Path) -> Option<String> {
    if !is_loop_file_sentinel(sentinel) {
        return None;
    }
    if !is_loop_default_prompt_enabled() {
        return None;
    }
    let dynamic = sentinel == LOOP_FILE_DYNAMIC_SENTINEL;
    let file = read_loop_file(cwd);
    let mut st = DELIVERY.lock().unwrap();
    if let Some(file) = file {
        // PARITY: `let o=t?V4d():G4d();if(Gst===n.content)return o;Gst=n.content,…`
        let tick = if dynamic {
            tick_loopfile_dynamic()
        } else {
            tick_loopfile_cron()
        };
        if st.last_content.as_deref() == Some(file.content.as_str()) {
            return Some(tick);
        }
        st.last_content = Some(file.content.clone());
        return Some(format!(
            "# /loop tick — tasks from {path}\nThe user configured a loop-tasks file. Work through the tasks defined below; these are the instructions for this tick and every subsequent tick (the reminder on later fires refers back to this message).\n{content}\n{tick}",
            path = file.path.display(),
            content = file.content,
        ));
    }
    // PARITY: loop.md ABSENT — `sKi` calls `pJr()` here (only on the absent path,
    // after the `if(n){…}` present-block), then `let r=t?K4d():tKi();
    // if(Gst===ZVi||iFt)return r;return Gst=ZVi,iFt=!0,`${dJr()}\n${r}``
    // (cc_all.txt:504966).
    log_autonomous_loop_activation();
    let tick = if dynamic {
        tick_loopfile_absent_dynamic()
    } else {
        tick_autonomous_cron()
    };
    if st.last_content.as_deref() == Some(PREAMBLE_SENTINEL) || st.preamble_delivered {
        return Some(tick);
    }
    st.last_content = Some(PREAMBLE_SENTINEL.to_string());
    st.preamble_delivered = true;
    Some(format!("{}\n{}", get_autonomous_loop_preamble(), tick))
}

/// `J4d` / `resolveLoopDefaultFire` (cc_all.txt:504966):
/// `nKi(e) ?? sKi(e) ?? e` — try autonomous, then loop.md, else passthrough.
#[must_use]
pub fn resolve_loop_default_fire(sentinel: &str, cwd: &Path) -> String {
    resolve_autonomous_loop_fire(sentinel)
        .or_else(|| resolve_loop_file_fire(sentinel, cwd))
        .unwrap_or_else(|| sentinel.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests touch the process-global DELIVERY state + env var, so serialize them
    // via the crate-shared lock (shared with `wakeup::tests` resolution tests).
    fn guard() -> std::sync::MutexGuard<'static, ()> {
        let g = super::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        reset_autonomous_loop_delivered();
        std::env::remove_var("CLAUDE_CODE_LOOP_PERSISTENT");
        // The resolver gate (`fJr`/`is_loop_default_prompt_enabled`) DEFAULTS off
        // (binary `tengu_kairos_loop_prompt=false`); turn it on for the resolution
        // tests via the port's `CLAUDE_CODE_LOOP_PROMPT` env override. The
        // dedicated `gate_off_passthrough` test removes it to assert the default.
        std::env::set_var("CLAUDE_CODE_LOOP_PROMPT", "1");
        g
    }

    #[test]
    fn sentinel_constants_locked() {
        let _g = guard();
        assert_eq!(AUTONOMOUS_LOOP_SENTINEL, "<<autonomous-loop>>");
        assert_eq!(AUTONOMOUS_LOOP_DYNAMIC_SENTINEL, "<<autonomous-loop-dynamic>>");
        assert_eq!(LOOP_FILE_SENTINEL, "<<loop.md>>");
        assert_eq!(LOOP_FILE_DYNAMIC_SENTINEL, "<<loop.md-dynamic>>");
    }

    #[test]
    fn sentinel_predicates() {
        let _g = guard();
        assert!(is_autonomous_loop_sentinel("<<autonomous-loop>>"));
        assert!(is_autonomous_loop_sentinel("<<autonomous-loop-dynamic>>"));
        assert!(!is_autonomous_loop_sentinel("<<loop.md>>"));
        assert!(is_loop_file_sentinel("<<loop.md>>"));
        assert!(is_loop_file_sentinel("<<loop.md-dynamic>>"));
        assert!(is_loop_default_sentinel("<<autonomous-loop-dynamic>>"));
        assert!(is_loop_default_sentinel("<<loop.md>>"));
        assert!(!is_loop_default_sentinel("5m /foo"));
    }

    #[test]
    fn preamble_default_is_ajr_verbatim() {
        let _g = guard();
        assert_eq!(AUTONOMOUS_LOOP_PREAMBLE, PREAMBLE_DEFAULT);
        assert!(PREAMBLE_DEFAULT.starts_with("# Autonomous loop check\n"));
        assert!(PREAMBLE_DEFAULT.contains("You're a steward, not an initiator."));
        assert!(PREAMBLE_DEFAULT.ends_with("building together.\n"));
        // The default preamble lacks the persistent-only "spirit" / "keep the
        // loop alive" phrasing.
        assert!(!PREAMBLE_DEFAULT.contains("the *spirit* of the task"));
    }

    #[test]
    fn preamble_persistent_selected_by_env() {
        let _g = guard();
        assert_eq!(get_autonomous_loop_preamble(), PREAMBLE_DEFAULT);
        std::env::set_var("CLAUDE_CODE_LOOP_PERSISTENT", "1");
        assert!(is_loop_persistent_preamble_enabled());
        assert_eq!(get_autonomous_loop_preamble(), PREAMBLE_PERSISTENT);
        assert!(PREAMBLE_PERSISTENT.contains("the *spirit* of the task"));
        assert!(PREAMBLE_PERSISTENT.contains("Persistence is the point of autonomous mode."));
        std::env::remove_var("CLAUDE_CODE_LOOP_PERSISTENT");
    }

    #[test]
    fn autonomous_dynamic_first_then_subsequent() {
        let _g = guard();
        // First fire: full preamble + dynamic tick.
        let first = resolve_autonomous_loop_fire(AUTONOMOUS_LOOP_DYNAMIC_SENTINEL).unwrap();
        assert!(first.starts_with("# Autonomous loop check\n"));
        assert!(first.contains("# Autonomous loop tick (dynamic pacing)"));
        assert!(first.contains("set to the literal sentinel `<<autonomous-loop-dynamic>>`"));
        // Subsequent fire: short tick only, no preamble.
        let second = resolve_autonomous_loop_fire(AUTONOMOUS_LOOP_DYNAMIC_SENTINEL).unwrap();
        assert!(!second.starts_with("# Autonomous loop check"));
        assert!(second.starts_with("# Autonomous loop tick (dynamic pacing)"));
    }

    #[test]
    fn autonomous_cron_tick_says_do_not_call_schedulewakeup() {
        let _g = guard();
        let out = resolve_autonomous_loop_fire(AUTONOMOUS_LOOP_SENTINEL).unwrap();
        assert!(out.contains("# Autonomous loop tick\n"));
        assert!(out.contains("do not call ScheduleWakeup from this tick."));
        // Reset state then a subsequent cron fire drops the preamble.
        let second = resolve_autonomous_loop_fire(AUTONOMOUS_LOOP_SENTINEL).unwrap();
        assert!(second.starts_with("# Autonomous loop tick\n"));
    }

    #[test]
    fn non_sentinel_passes_through() {
        let _g = guard();
        let cwd = std::env::temp_dir();
        assert_eq!(resolve_loop_default_fire("5m /babysit-prs", &cwd), "5m /babysit-prs");
        assert!(resolve_autonomous_loop_fire("5m /x").is_none());
        assert!(resolve_loop_file_fire("5m /x", &cwd).is_none());
    }

    #[test]
    fn gate_off_passthrough() {
        // PARITY: with `fJr()`/`is_loop_default_prompt_enabled()` at its binary
        // default (`tengu_kairos_loop_prompt=false`), the resolver returns the
        // sentinel UNCHANGED — `J4d(e)=nKi(e)??sKi(e)??e` → `e`. This is the exact
        // shipped-binary behavior (the feature is gated off until the server flag
        // flips). `guard()` sets the override; remove it to exercise the default.
        let _g = guard();
        std::env::remove_var("CLAUDE_CODE_LOOP_PROMPT");
        assert!(!is_loop_default_prompt_enabled());
        let cwd = std::env::temp_dir();
        // All four sentinels pass through verbatim when the gate is off.
        assert_eq!(
            resolve_loop_default_fire(AUTONOMOUS_LOOP_DYNAMIC_SENTINEL, &cwd),
            AUTONOMOUS_LOOP_DYNAMIC_SENTINEL
        );
        assert_eq!(
            resolve_loop_default_fire(AUTONOMOUS_LOOP_SENTINEL, &cwd),
            AUTONOMOUS_LOOP_SENTINEL
        );
        assert_eq!(
            resolve_loop_default_fire(LOOP_FILE_DYNAMIC_SENTINEL, &cwd),
            LOOP_FILE_DYNAMIC_SENTINEL
        );
        assert!(resolve_autonomous_loop_fire(AUTONOMOUS_LOOP_DYNAMIC_SENTINEL).is_none());
        assert!(resolve_loop_file_fire(LOOP_FILE_SENTINEL, &cwd).is_none());
    }

    #[test]
    fn loop_file_first_inlines_then_reminds() {
        let _g = guard();
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("loop.md"), "- task A\n- task B\n").unwrap();
        // First fire: inlines the full loop.md contents (NO autonomous preamble —
        // the binary's sKi prefixes the preamble only on the loop.md-ABSENT path).
        let first = resolve_loop_file_fire(LOOP_FILE_DYNAMIC_SENTINEL, tmp.path()).unwrap();
        assert!(!first.contains("# Autonomous loop check"));
        assert!(first.starts_with("# /loop tick — tasks from "));
        assert!(first.contains("- task A"));
        assert!(first.contains("# /loop tick — loop.md tasks (dynamic pacing)"));
        // Second fire, unchanged content: short reminder tick only.
        let second = resolve_loop_file_fire(LOOP_FILE_DYNAMIC_SENTINEL, tmp.path()).unwrap();
        assert!(!second.contains("# Autonomous loop check"));
        // No "tasks from <path>" inline-block header on the unchanged reminder.
        assert!(!second.contains("# /loop tick — tasks from "));
        assert!(!second.contains("- task A"));
        assert!(second.starts_with("# /loop tick — loop.md tasks (dynamic pacing)"));
        // Edited content re-inlines.
        std::fs::write(tmp.path().join("loop.md"), "- task C\n").unwrap();
        let third = resolve_loop_file_fire(LOOP_FILE_DYNAMIC_SENTINEL, tmp.path()).unwrap();
        assert!(third.contains("# /loop tick — tasks from "));
        assert!(third.contains("- task C"));
    }

    #[test]
    fn loop_file_absent_falls_back_to_autonomous() {
        let _g = guard();
        let tmp = tempfile::tempdir().unwrap();
        // No loop.md present: first dynamic fire → preamble + absent-tick.
        let first = resolve_loop_file_fire(LOOP_FILE_DYNAMIC_SENTINEL, tmp.path()).unwrap();
        assert!(first.starts_with("# Autonomous loop check\n"));
        assert!(first.contains("# /loop tick — loop.md absent (dynamic pacing)"));
        // Subsequent absent fire: short absent-tick, no preamble.
        let second = resolve_loop_file_fire(LOOP_FILE_DYNAMIC_SENTINEL, tmp.path()).unwrap();
        assert!(!second.contains("# Autonomous loop check"));
        assert!(second.starts_with("# /loop tick — loop.md absent (dynamic pacing)"));
    }

    #[test]
    fn loop_file_truncation() {
        let _g = guard();
        let big = "x".repeat(LOOP_FILE_MAX_BYTES + 5000);
        let out = truncate_loop_file(&big);
        assert!(out.len() <= LOOP_FILE_MAX_BYTES + 200);
        assert!(out.ends_with(
            "> WARNING: loop.md was truncated to 25000 bytes. Keep the task list concise."
        ));
        // Short content is untouched.
        assert_eq!(truncate_loop_file("- task\n"), "- task\n");
    }

    /// Regression: a multibyte UTF-8 char straddling byte 25000 must NOT panic
    /// (Rust string slicing panics on a non-char-boundary index; the binary's
    /// `z4d` JS `.slice`/`lastIndexOf` operate on UTF-16 code units and never
    /// panic). The fix clamps the budget DOWN to the nearest char boundary.
    #[test]
    fn loop_file_truncation_multibyte_boundary_no_panic() {
        let _g = guard();
        // `€` is 3 bytes (E2 82 AC). 8334 * 3 = 25002 bytes > 25000, with a char
        // boundary at 24999 and the next at 25002 — so byte 25000 lands INSIDE a
        // char. There are no newlines, so the rfind fallback hits the clamped
        // budget. This panicked before the fix.
        let big = "\u{20ac}".repeat(8334);
        assert!(big.len() > LOOP_FILE_MAX_BYTES);
        let out = truncate_loop_file(&big);
        // The head is valid UTF-8 (never split a char) and the footer is appended.
        assert!(out.ends_with(
            "> WARNING: loop.md was truncated to 25000 bytes. Keep the task list concise."
        ));
        // Every retained head char is the full `€` (no replacement/mojibake).
        let head = out
            .strip_suffix(
                "\n> WARNING: loop.md was truncated to 25000 bytes. Keep the task list concise.",
            )
            .expect("footer present");
        assert!(head.chars().all(|c| c == '\u{20ac}'));
        // Boundary-clamped: head is the largest whole-char prefix <= 25000 bytes.
        assert!(head.len() <= LOOP_FILE_MAX_BYTES);
        assert!(head.len() >= LOOP_FILE_MAX_BYTES - 3);
    }

    #[test]
    fn push_notif_addendum_off_by_default() {
        let _g = guard();
        assert_eq!(push_notif_addendum(true), "");
        assert_eq!(push_notif_addendum(false), "");
    }

    #[test]
    fn dynamic_tick_includes_monitor_addendum() {
        let _g = guard();
        let out = tick_autonomous_dynamic();
        assert!(out.contains("If a Monitor is armed (check TaskList)"));
        assert!(out.contains("TaskStop the monitor"));
    }

    #[test]
    fn reset_clears_delivery_state() {
        let _g = guard();
        let _ = resolve_autonomous_loop_fire(AUTONOMOUS_LOOP_DYNAMIC_SENTINEL);
        // After a fire the preamble is marked delivered.
        reset_autonomous_loop_delivered();
        // Next fire re-emits the preamble.
        let again = resolve_autonomous_loop_fire(AUTONOMOUS_LOOP_DYNAMIC_SENTINEL).unwrap();
        assert!(again.starts_with("# Autonomous loop check\n"));
    }
}
