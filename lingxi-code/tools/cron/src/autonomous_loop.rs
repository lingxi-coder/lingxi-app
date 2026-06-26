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
//! FEATURE FLAGS: every gate reads the binary's sync flag reader `nt(key,default)`
//! — ported as [`telemetry::flag_bool`] (an empty cached snapshot by default →
//! returns `default`, i.e. the shipped binary's GrowthBook-absent behavior) —
//! with the binary's EXACT per-gate env-vs-flag split:
//!   - resolution gate `isLoopDefaultPromptEnabled` = `nt("tengu_kairos_loop_prompt",false)` (FLAG-ONLY)
//!   - dynamic gate `isLoopDynamic` = `nt("tengu_kairos_loop_dynamic",false)` (FLAG-ONLY)
//!   - preamble variant `isLoopPersistentPreambleEnabled` = env `CLAUDE_CODE_LOOP_PERSISTENT` || `nt("tengu_kairos_loop_persistent",false)`
//!   - keepalive gate `isLoopKeepaliveEnabled` = env `CLAUDE_CODE_LOOP_KEEPALIVE` || `nt("tengu_kairos_loop_keepalive",false)`
//!   - `PushNotification` addendum `Yke()` = `nt("tengu_kairos_push_notifications",false)` && `agentPushNotifEnabled` setting
//! With no live GrowthBook fetcher wired (the prod default) every flag is at its
//! shipped `false`, so the whole subsystem is inert and byte-identical to the
//! shipped binary. Tests flip a flag via [`telemetry::test_set_flag`] (binary's
//! `ROt`/`Uvi` override layer) rather than env vars. NOTE: the earlier
//! `CLAUDE_CODE_LOOP_PROMPT`/`CLAUDE_CODE_LOOP_DYNAMIC` env stand-ins were REMOVED
//! — the binary's `fJr`/`q_e` are flag-only (no env layer), so those envs were a
//! false-positive divergence. Only PERSISTENT/KEEPALIVE keep an env layer (the
//! binary's `YIn`/`iKi` genuinely have one).

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

// ── Feature-flag gates (binary `nt` flag reads + per-gate env overrides) ──────
//
// The binary's sync flag reader `nt(key,default)` is ported as
// `telemetry::flag_bool` (an empty cached snapshot by default → returns the
// passed default = the shipped binary's GrowthBook-absent behavior). Each gate
// below matches the binary's EXACT env-vs-flag split: PERSISTENT/KEEPALIVE are
// `env || flag` (the binary's `YIn`/`iKi` have a `CLAUDE_CODE_LOOP_*` env layer);
// PROMPT/DYNAMIC are FLAG-ONLY (`fJr`/`q_e` have NO env layer in the binary).

/// `YIn` / `isLoopPersistentPreambleEnabled` (cc_all.txt:504950):
/// `rt(process.env.CLAUDE_CODE_LOOP_PERSISTENT) || nt("tengu_kairos_loop_persistent",false)`.
#[must_use]
pub fn is_loop_persistent_preamble_enabled() -> bool {
    env_truthy("CLAUDE_CODE_LOOP_PERSISTENT")
        || telemetry::flag_bool("tengu_kairos_loop_persistent", false)
}

/// `fJr` / `isLoopDefaultPromptEnabled` (cc_all.txt:504952):
/// `nt("tengu_kairos_loop_prompt",false)` — FLAG ONLY (no env in the binary).
/// Gates whether the autonomous/loop.md sentinels resolve at all (else `J4d`
/// passes them through verbatim). With no live GrowthBook the flag is at its
/// shipped default `false`, so the resolvers pass sentinels through unchanged —
/// byte-identical to the shipped binary. Tests flip it via
/// `telemetry::test_set_flag("tengu_kairos_loop_prompt", true)`.
#[must_use]
pub fn is_loop_default_prompt_enabled() -> bool {
    telemetry::flag_bool("tengu_kairos_loop_prompt", false)
}

/// `q_e` / `isLoopDynamic` (cc_all.txt:504966):
/// `nt("tengu_kairos_loop_dynamic",false)` — FLAG ONLY (no env in the binary).
/// Selects the DYNAMIC-pacing builders (`hZm` usage / `gZm` prompt-builder, and
/// `a(loopFile,true)` for the no-prompt autonomous default) over the cron variants.
#[must_use]
pub fn is_loop_dynamic_enabled() -> bool {
    telemetry::flag_bool("tengu_kairos_loop_dynamic", false)
}

/// `iKi` / `isLoopKeepaliveEnabled` (cc_all.txt:504966):
/// `rt(process.env.CLAUDE_CODE_LOOP_KEEPALIVE) || nt("tengu_kairos_loop_keepalive",false)`.
/// Gates the keepalive fallback heartbeat (the `lKi`/`cKi` re-arm when a dynamic
/// loop tick completes without the model rescheduling).
// PARITY: the keepalive *gate* is ported here; the keepalive *scheduling*
// machinery (`lKi`/`cKi`, the in-flight-tick tagging, the consecutive-keepalive
// budget, and the loading→idle trigger) is a separate larger subsystem still
// pending — so this gate currently has no production consumer beyond the gate
// being available for that follow-on work.
#[must_use]
pub fn is_loop_keepalive_enabled() -> bool {
    env_truthy("CLAUDE_CODE_LOOP_KEEPALIVE")
        || telemetry::flag_bool("tengu_kairos_loop_keepalive", false)
}

/// `Yke` (cc_all.txt:504927): `Rle() && mc("agentPushNotifEnabled",false).value`,
/// where `Rle()` = `nt("tengu_kairos_push_notifications",false)`. Gates the
/// `PushNotification` addendum appended to tick prompts.
// PARITY: the flag `tengu_kairos_push_notifications` is read via `flag_bool`
// (default false, no live GrowthBook) and the `agentPushNotifEnabled` setting is
// not a supported port setting (binary `mc(...,false)` default → false), so
// `Yke()` resolves to `false` and `aFt()`/`Kpc()` render "" — byte-identical to
// the shipped binary's default config. The gate is now STRUCTURALLY 1:1 (it reads
// the real flag) rather than a hardcoded `false`, so it flips correctly if the
// flag/setting are ever enabled.
#[must_use]
pub fn is_push_notif_enabled() -> bool {
    telemetry::flag_bool("tengu_kairos_push_notifications", false) && agent_push_notif_setting()
}

/// `mc("agentPushNotifEnabled", false).value` (cc_all.txt:504927). The port has
/// no supported `agentPushNotifEnabled` setting (see `tools/meta` config — it is
/// deliberately excluded), so this returns the binary default `false`.
// PARITY-TODO: read the real `agentPushNotifEnabled` setting once the
// notification-settings subsystem (Tier 2) is ported.
#[must_use]
fn agent_push_notif_setting() -> bool {
    false
}

fn env_truthy(key: &str) -> bool {
    // PARITY: binary `rt(e)` (cc_all.txt) is an ALLOWLIST, not a denylist:
    // `String(e).toLowerCase().trim()` must be exactly one of `1|true|yes|on`.
    // The workspace-canonical `traits::env::is_env_truthy` implements precisely
    // this (and is used by ~10 other gates), so delegate to it — an earlier
    // denylist here wrongly treated `no`/`off`/`2`/`foo` as truthy.
    traits::env::is_env_truthy(std::env::var(key).ok().as_deref())
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
/// first-delivery state so the next fire re-emits the full preamble.
// PARITY: in the binary `X4d` is invoked from the post-compact cleanup `Zne`
// (`if(o)resetAutonomousLoopDelivered()`, main-thread compact). The port wires it
// at the same site — `compaction::run_post_compact_cleanup` inside its
// main-thread-compact gate. Inert by default (the resolver gate
// `tengu_kairos_loop_prompt` is off → `DELIVERY` is never mutated), so it
// byte-matches the shipped binary until the flag flips.
pub fn reset_autonomous_loop_delivered() {
    let mut st = DELIVERY.lock().unwrap();
    st.preamble_delivered = false;
    st.last_content = None;
}

// ── Loop runtime state (binary `Nt.loopTickInFlightPrompt` / `…Keepalives`) ───
//
// The keepalive fallback tracks, across a /loop tick turn: the prompt of the tick
// currently in flight (`tAt`/`I7e`) and how many consecutive keepalives have been
// armed without the model rescheduling (`PZt`/`nAt`, budget `tqd`=1). In the
// binary these live on the session state object `Nt`; the port mirrors them with
// a process-global [`Mutex`] (single live /loop per process, like `DELIVERY`).

#[derive(Default)]
struct LoopRuntimeState {
    /// `Nt.loopTickInFlightPrompt` — prompt of the loop tick being processed.
    tick_in_flight_prompt: Option<String>,
    /// `Nt.loopConsecutiveKeepalives` — consecutive keepalive count.
    consecutive_keepalives: u32,
    /// The port's stand-in for `Xke()` (is-a-loop-cron-armed): set when the model
    /// calls `ScheduleWakeup` successfully this turn. The port has no loop-cron
    /// registry to query, and the wakeup enqueues in the FUTURE, so this
    /// synchronous per-turn flag is the only reliable "model rescheduled" signal.
    rescheduled_this_turn: bool,
}

static LOOP_RUNTIME: Mutex<LoopRuntimeState> = Mutex::new(LoopRuntimeState {
    tick_in_flight_prompt: None,
    consecutive_keepalives: 0,
    rescheduled_this_turn: false,
});

/// Mark the START of a loop-tick turn (binary `onFireTask` `I7e(d.prompt)`):
/// record the in-flight tick prompt and clear the per-turn reschedule flag. Called
/// by the bridge drain when it pops a `QueueSource::Cron` command.
pub fn begin_loop_tick(prompt: String) {
    let mut st = LOOP_RUNTIME.lock().unwrap();
    st.tick_in_flight_prompt = Some(prompt);
    st.rescheduled_this_turn = false;
}

/// `tAt` (cc_all.txt) — peek the in-flight loop-tick prompt (or None).
#[must_use]
pub fn loop_tick_in_flight_prompt() -> Option<String> {
    LOOP_RUNTIME.lock().unwrap().tick_in_flight_prompt.clone()
}

/// `I7e(null)` (cc_all.txt) at turn end — take (read+clear) the in-flight prompt.
/// `Some` iff the just-completed turn was a loop tick.
pub fn take_loop_tick_in_flight_prompt() -> Option<String> {
    LOOP_RUNTIME.lock().unwrap().tick_in_flight_prompt.take()
}

/// The port's `Xke()=true` side: record that the model rescheduled this turn
/// (`ScheduleWakeup` success). Called from `ScheduleWakeupTool::call`.
pub fn mark_loop_rescheduled() {
    LOOP_RUNTIME.lock().unwrap().rescheduled_this_turn = true;
}

/// Take (read+clear) the per-turn reschedule flag — the port's `!Xke()` check.
pub fn take_loop_rescheduled() -> bool {
    let mut st = LOOP_RUNTIME.lock().unwrap();
    std::mem::take(&mut st.rescheduled_this_turn)
}

/// `PZt` (cc_all.txt) — consecutive-keepalive count.
#[must_use]
pub fn loop_consecutive_keepalives() -> u32 {
    LOOP_RUNTIME.lock().unwrap().consecutive_keepalives
}

/// `nAt` (cc_all.txt) — set the consecutive-keepalive count. Reset to 0 on any
/// non-keepalive schedule (binary `cKi`: `if(!r)nAt(0)`).
pub fn set_loop_consecutive_keepalives(n: u32) {
    LOOP_RUNTIME.lock().unwrap().consecutive_keepalives = n;
}

/// Clear all loop runtime state. Used by tests and a fresh-loop start.
pub fn reset_loop_runtime_state() {
    let mut st = LOOP_RUNTIME.lock().unwrap();
    st.tick_in_flight_prompt = None;
    st.consecutive_keepalives = 0;
    st.rescheduled_this_turn = false;
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
        telemetry::test_clear_flag("tengu_kairos_loop_persistent");
        // The resolver gate (`fJr`/`is_loop_default_prompt_enabled`) DEFAULTS off
        // (binary `tengu_kairos_loop_prompt=false`, FLAG-ONLY — no env). Turn it on
        // for the resolution tests via the test-only flag override (binary `ROt`/
        // `Uvi`). The dedicated `gate_off_passthrough` test clears it to assert the
        // default.
        telemetry::test_set_flag("tengu_kairos_loop_prompt", true);
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
    fn gate_env_vs_flag_split_matches_binary() {
        let _g = guard();
        // guard() set the prompt flag on; clear all loop flags + envs for a clean
        // baseline (shipped-binary default: every gate off).
        telemetry::test_clear_flag("tengu_kairos_loop_prompt");
        telemetry::test_clear_flag("tengu_kairos_loop_dynamic");
        telemetry::test_clear_flag("tengu_kairos_loop_persistent");
        telemetry::test_clear_flag("tengu_kairos_loop_keepalive");
        telemetry::test_clear_flag("tengu_kairos_push_notifications");
        std::env::remove_var("CLAUDE_CODE_LOOP_PERSISTENT");
        std::env::remove_var("CLAUDE_CODE_LOOP_KEEPALIVE");
        assert!(!is_loop_default_prompt_enabled());
        assert!(!is_loop_dynamic_enabled());
        assert!(!is_loop_persistent_preamble_enabled());
        assert!(!is_loop_keepalive_enabled());
        assert!(!is_push_notif_enabled());

        // PROMPT/DYNAMIC: FLAG-ONLY (binary fJr/q_e have NO env layer).
        telemetry::test_set_flag("tengu_kairos_loop_prompt", true);
        telemetry::test_set_flag("tengu_kairos_loop_dynamic", true);
        assert!(is_loop_default_prompt_enabled());
        assert!(is_loop_dynamic_enabled());
        // The removed env vars must NOT influence the flag-only gates.
        telemetry::test_clear_flag("tengu_kairos_loop_prompt");
        std::env::set_var("CLAUDE_CODE_LOOP_PROMPT", "1");
        assert!(
            !is_loop_default_prompt_enabled(),
            "CLAUDE_CODE_LOOP_PROMPT must NOT enable the flag-only gate (binary fJr is flag-only)"
        );
        std::env::remove_var("CLAUDE_CODE_LOOP_PROMPT");
        telemetry::test_clear_flag("tengu_kairos_loop_dynamic");

        // PERSISTENT/KEEPALIVE: env || flag (binary YIn/iKi have both).
        telemetry::test_set_flag("tengu_kairos_loop_persistent", true);
        assert!(is_loop_persistent_preamble_enabled(), "flag arm");
        telemetry::test_clear_flag("tengu_kairos_loop_persistent");
        assert!(!is_loop_persistent_preamble_enabled());
        std::env::set_var("CLAUDE_CODE_LOOP_PERSISTENT", "1");
        assert!(is_loop_persistent_preamble_enabled(), "env arm");
        std::env::remove_var("CLAUDE_CODE_LOOP_PERSISTENT");

        telemetry::test_set_flag("tengu_kairos_loop_keepalive", true);
        assert!(is_loop_keepalive_enabled(), "flag arm");
        telemetry::test_clear_flag("tengu_kairos_loop_keepalive");
        std::env::set_var("CLAUDE_CODE_LOOP_KEEPALIVE", "1");
        assert!(is_loop_keepalive_enabled(), "env arm");
        std::env::remove_var("CLAUDE_CODE_LOOP_KEEPALIVE");

        // Yke: push flag alone is not enough (agentPushNotifEnabled setting is
        // unsupported → false), so Yke stays false — matching the binary default.
        telemetry::test_set_flag("tengu_kairos_push_notifications", true);
        assert!(
            !is_push_notif_enabled(),
            "Yke needs BOTH the flag and the agentPushNotifEnabled setting"
        );
        telemetry::test_clear_flag("tengu_kairos_push_notifications");
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
        // flips). `guard()` sets the override; clear it to exercise the default.
        let _g = guard();
        telemetry::test_clear_flag("tengu_kairos_loop_prompt");
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
