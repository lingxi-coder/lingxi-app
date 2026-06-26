//! Bundled `/loop` skill — 1:1 port of the 2.1.191 binary's `/loop`
//! `getPromptForCommand` builder.
//!
//! The binary registers `/loop` via `_Zm()` (cc_all.txt:521920) with a
//! `getPromptForCommand(e,t)` that dispatches on TWO feature flags:
//!   - `q_e()` = `tengu_kairos_loop_dynamic` (default **false**)
//!   - `isLoopDefaultPromptEnabled()` = `tengu_kairos_loop_prompt` (default false)
//!
//! The FULL dispatch matrix is now ported (see [`LoopPromptFn::build`]):
//!   1. no-prompt (empty/interval-only) && `isLoopDefaultPromptEnabled()` → the
//!      autonomous-default builder `a(loopFile, dynamic)` (binary inline `a=(c,u)`)
//!   2. `q_e()` → `hZm()` usage (empty) / `gZm(n)` prompt builder
//!   3. default (BOTH flags off, the SHIPPED binary) → `dZm` usage (empty) /
//!      `fZm(n)` cron prompt (cc_all.txt:521946)
//!
//! The two `tengu_kairos_loop_*` flags are read through the sync flag reader
//! [`telemetry::flag_bool`] (the port's `nt`); with no live GrowthBook fetcher
//! wired they sit at the binary's shipped `false`, so only branch 3 is reachable
//! — byte-identical to the shipped binary. (Unlike PERSISTENT/KEEPALIVE, the
//! PROMPT/DYNAMIC gates are FLAG-ONLY — the binary `fJr`/`q_e` have no env layer,
//! so there is no `LINGXI_LOOP_PROMPT`/`_DYNAMIC` env; tests flip the flags
//! via `telemetry::test_set_flag`.) The flag-on builders live in
//! [`tool_cron`]'s `autonomous_loop` module (loop.md detection, the preamble, the
//! sentinels, `logAutonomousLoopActivation`), imported here exactly as the binary
//! loop command imports `QVe = io(T3e)`. The cloud-offer / push-notification
//! splices (`zpc`/`Ypc`/`Kpc`) are gated on `tengu_surreal_dali` /
//! `allow_remote_sessions` / push-notif (all default off, no port subsystem) and
//! render "" — matching the default-disabled binary path.
//!
//! Literal interpolations resolve to constants here: `${VSt}` = `10m`,
//! `${xw}` = `CronCreate`, `${t9}` = `CronDelete`, `${lte}` = `30`
//! (`DEFAULT_MAX_AGE_DAYS`), `${Kh}` = `ScheduleWakeup`, `${IA}` = `Monitor`,
//! `${AI}` = `TaskList`, `${eP}` = `TaskStop`.

use command_api::BundledPromptFn;

/// Binary `VSt` (cc_all.txt:521947) — default interval when none is parsed.
const DEFAULT_INTERVAL: &str = "10m";

/// Binary `dZm` (cc_all.txt:521947) — returned verbatim when the trimmed args
/// are empty (or interval-only). `${VSt}` → `10m`. No trailing newline (the
/// binary template literal ends at the last example line).
// PARITY: binary dZm (cc_all.txt:521947); matches BLOCK D (binary-loop-reference).
const USAGE_MESSAGE: &str = "Usage: /loop [interval] <prompt>
Run a prompt or slash command on a recurring interval.
Intervals: Ns, Nm, Nh, Nd (e.g. 5m, 30m, 2h, 1d). Minimum granularity is 1 minute.
If no interval is specified, defaults to 10m.
Examples:
  /loop 5m /babysit-prs
  /loop 30m check the deploy
  /loop 1h /standup 1
  /loop check the deploy          (defaults to 10m)
  /loop check the deploy every 20m";

/// Binary `hZm` (cc_all.txt:521879) — the USAGE message for the DYNAMIC variant,
/// returned by dispatch branch 2 when `q_e()` = `tengu_kairos_loop_dynamic`
/// (flag-only) is on and the input is empty.
// PARITY: binary hZm (cc_all.txt:521879-521889).
const USAGE_MESSAGE_DYNAMIC: &str = "Usage: /loop [interval] <prompt>
Run a prompt or slash command on a recurring interval — or with no interval, let the model self-pace based on the task.
Intervals: Ns, Nm, Nh, Nd (e.g. 5m, 30m, 2h, 1d). Minimum granularity is 1 minute.
If no interval is specified, the model picks a delay between iterations based on what it's doing.
Examples:
  /loop 5m /babysit-prs
  /loop 30m check the deploy
  /loop 1h /standup 1
  /loop check the deploy          (dynamic — model picks delays)
  /loop check the deploy every 20m";

/// Binary `fZm(e)` head (cc_all.txt:521800-521846) — everything before the final
/// `## Input\n${e}`. BYTE-EXACT to the shipped binary with the cloud-offer
/// splices DISABLED. The 2.1.191 binary collapsed the older leaked `loop.ts`
/// blank-line spacing to SINGLE newlines everywhere EXCEPT the `${zpc()}` splice
/// point: the binary emits `…→ show usage\n${zpc()}\n## Interval → cron`, so when
/// `zpc()` returns `""` (the default) the result is `…→ show usage\n\n## Interval`
/// — a preserved blank line, which this const reproduces. Interpolations:
/// `${xw}` = `CronCreate`, `${VSt}` = `10m`, `${lte}` = `30`, `${t9}` = `CronDelete`.
///
// PARITY: binary fZm (cc_all.txt:521800-521846).
// PARITY-TODO: fZm splices `${zpc()}` (own line, after the parsing examples) and
// `${Ypc()}` (inline, after the confirm step `${t9} (include the job ID).${Ypc()}`)
// — cloud-offer / "Runs until you close this session" lines gated on
// `tengu_surreal_dali` + `allow_remote_sessions` (cc_all.txt:521830/521844). Both
// default OFF and BOTH guards end with `return""` (verified), and the port has no
// remote-session subsystem, so both render "" — this const matches the default-
// disabled binary path: `zpc()` leaves a blank line before `## Interval` (its own
// line collapses to `\n\n`); `Ypc()` is inline so it leaves a single `\n` before
// step 3.
fn cron_prompt_head() -> String {
    format!(
        "# /loop — schedule a recurring prompt
Parse the input below into `[interval] <prompt…>` and schedule it with CronCreate.
## Parsing (in priority order)
1. **Leading token**: if the first whitespace-delimited token matches `^\\d+[smhd]$` (e.g. `5m`, `2h`), that's the interval; the rest is the prompt.
2. **Trailing \"every\" clause**: otherwise, if the input ends with `every <N><unit>` or `every <N> <unit-word>` (e.g. `every 20m`, `every 5 minutes`, `every 2 hours`), extract that as the interval and strip it from the prompt. Only match when what follows \"every\" is a time expression — `check every PR` has no interval.
3. **Default**: otherwise, interval is `{default}` and the entire input is the prompt.
If the resulting prompt is empty, show usage `/loop [interval] <prompt>` and stop — do not call CronCreate.
Examples:
- `5m /babysit-prs` → interval `5m`, prompt `/babysit-prs` (rule 1)
- `check the deploy every 20m` → interval `20m`, prompt `check the deploy` (rule 2)
- `run tests every 5 minutes` → interval `5m`, prompt `run tests` (rule 2)
- `check the deploy` → interval `{default}`, prompt `check the deploy` (rule 3)
- `check every PR` → interval `{default}`, prompt `check every PR` (rule 3 — \"every\" not followed by time)
- `5m` → empty prompt → show usage

## Interval → cron
Supported suffixes: `s` (seconds, rounded up to nearest minute, min 1), `m` (minutes), `h` (hours), `d` (days). Convert:
| Interval pattern      | Cron expression     | Notes                                    |
|-----------------------|---------------------|------------------------------------------|
| `Nm` where N ≤ 59   | `*/N * * * *`     | every N minutes                          |
| `Nm` where N ≥ 60   | `0 */H * * *`     | round to hours (H = N/60, must divide 24)|
| `Nh` where N ≤ 23   | `0 */N * * *`     | every N hours                            |
| `Nd`                | `0 0 */N * *`     | every N days at midnight local           |
| `Ns`                | treat as `ceil(N/60)m` | cron minimum granularity is 1 minute  |
**If the interval doesn't cleanly divide its unit** (e.g. `7m` → `*/7 * * * *` gives uneven gaps at :56→:00; `90m` → 1.5h which cron can't express), pick the nearest clean interval and tell the user what you rounded to before scheduling.
## Action
1. Call CronCreate with:
   - `cron`: the expression from the table above
   - `prompt`: the parsed prompt from above, verbatim (slash commands are passed through unchanged)
   - `recurring`: `true`
2. Briefly confirm: what's scheduled, the cron expression, the human-readable cadence, that recurring tasks auto-expire after 30 days, and that they can cancel sooner with CronDelete (include the job ID).
3. **Then immediately execute the parsed prompt now** — don't wait for the first cron fire. If it's a slash command, invoke it via the Skill tool; otherwise act on it directly.",
        default = DEFAULT_INTERVAL,
    )
}

/// Binary `fZm(e)` (cc_all.txt:521800) — `${head}\n## Input\n${e}` with `${e}`
/// the trimmed input. SINGLE newline before `## Input` (binary template).
// PARITY: binary fZm tail `## Input\n${e}` (cc_all.txt:521846).
fn build_prompt(args: &str) -> String {
    format!(
        "{head}\n## Input\n{args}",
        head = cron_prompt_head(),
        args = args,
    )
}

// ── Flag-on (autonomous-default / dynamic-pacing) builders ───────────────────
//
// These implement the binary `getPromptForCommand` branches gated on
// `isLoopDefaultPromptEnabled()` (`tengu_kairos_loop_prompt`) and `q_e()`
// (`tengu_kairos_loop_dynamic`) — both FLAG-ONLY (read via `telemetry::flag_bool`,
// no env layer). With both flags at their shipped default `false` (no live
// GrowthBook), the dispatch falls through to the cron variants (`dZm`/`fZm`) —
// byte-identical to the shipped binary.

use std::sync::OnceLock;

use regex::Regex;
use tool_cron::{
    get_autonomous_loop_preamble, is_loop_default_prompt_enabled, is_loop_dynamic_enabled,
    log_autonomous_loop_activation, read_loop_file, LoopFile, AUTONOMOUS_LOOP_DYNAMIC_SENTINEL,
    AUTONOMOUS_LOOP_SENTINEL, LOOP_FILE_DYNAMIC_SENTINEL, LOOP_FILE_SENTINEL,
};

// Tool-name interpolations (binary `Kh`/`IA`/`AI`/`eP`/`xw`/`t9`).
const SCHEDULE_WAKEUP: &str = "ScheduleWakeup"; // Kh
const MONITOR: &str = "Monitor"; // IA
const TASK_LIST: &str = "TaskList"; // AI
const TASK_STOP: &str = "TaskStop"; // eP
const CRON_CREATE: &str = "CronCreate"; // xw
const CRON_DELETE: &str = "CronDelete"; // t9
/// Binary `lte` (cc_all.txt:521920) — `DEFAULT_MAX_AGE_DAYS`.
const MAX_AGE_DAYS: u32 = 30;

/// Binary `lZm` (cc_all.txt:521947) — interval-only matcher `^\d+[smhd]$`.
fn interval_only_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\d+[smhd]$").unwrap())
}

/// Binary `cZm` (cc_all.txt:521947) — trailing "every <N> <unit>" matcher.
fn every_clause_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)^every\s+(\d+)\s*(s|sec|secs|second|seconds|m|min|mins|minute|minutes|h|hr|hrs|hour|hours|d|day|days)\s*$",
        )
        .unwrap()
    })
}

/// Binary `uZm(e)` (cc_all.txt:521920) — normalize a parsed `every` clause to a
/// canonical interval token: `s`→`Ns`, `h`→`Nh`, `d`→`Nd`, else `Nm`.
fn normalize_every(num: &str, unit: &str) -> String {
    let u = unit.to_ascii_lowercase();
    if u.starts_with('s') {
        format!("{num}s")
    } else if u.starts_with('h') {
        format!("{num}h")
    } else if u.starts_with('d') {
        format!("{num}d")
    } else {
        format!("{num}m")
    }
}

/// Binary `zpc()` (cc_all.txt:521830) — cloud-offer splice. Gated on
/// `tengu_surreal_dali` && `allow_remote_sessions` (both default off) and the
/// port has no remote-session subsystem → always "".
fn cloud_offer_block() -> &'static str {
    ""
}

/// Binary `Ypc()` (cc_all.txt:521844) — inline "Runs until you close this
/// session" confirmation line. Same gates as [`cloud_offer_block`] → "".
fn remote_confirm_line() -> &'static str {
    ""
}

/// Binary `Kpc()` (cc_all.txt:521844) — the `PushNotification` "send a one-line
/// outcome before you stop" splice: `return Yke()?" Before you stop, …":""`.
/// Gated on the shared `Yke()` (`tool_cron::is_push_notif_enabled`), so it is
/// structurally 1:1 — it renders "" only because the push-notif flag/setting
/// default off (no live GrowthBook), matching the shipped binary. Note the
/// leading space (the binary splices it inline after the step-6 sentence).
fn push_outcome_line() -> &'static str {
    if tool_cron::is_push_notif_enabled() {
        " Before you stop, send a one-line outcome via PushNotification — the user may be away and waiting to hear it's done. Skip this if you're stopping because the user just told you to; they're already here."
    } else {
        ""
    }
}

/// Binary `pZm` (cc_all.txt:521920) — the interval→cron conversion table, used by
/// the dynamic-flag `gZm` builder. Byte-exact incl. the trailing blank line +
/// rounding note (`≤`/`≥` are U+2264/U+2265; `→` is U+2192).
const CRON_TABLE: &str = "| Interval pattern      | Cron expression     | Notes                                    |
|-----------------------|---------------------|------------------------------------------|
| `Nm` where N ≤ 59   | `*/N * * * *`     | every N minutes                          |
| `Nm` where N ≥ 60   | `0 */H * * *`     | round to hours (H = N/60, must divide 24)|
| `Nh` where N ≤ 23   | `0 */N * * *`     | every N hours                            |
| `Nd`                | `0 0 */N * *`     | every N days at midnight local           |
| `Ns`                | treat as `ceil(N/60)m` | cron minimum granularity is 1 minute  |

**If the interval doesn't cleanly divide its unit** (e.g. `7m` → `*/7 * * * *` gives uneven gaps at :56→:00; `90m` → 1.5h which cron can't express), pick the nearest clean interval and tell the user what you rounded to before scheduling.";

/// Binary `mZm()` (cc_all.txt:521920) — the fixed-interval action block spliced
/// into `gZm`. `${Ypc()}` (inline, after step 2) renders "" by default.
fn fixed_interval_action() -> String {
    format!(
        "1. Call {CRON_CREATE} with: `cron` (the expression above), `prompt` (the parsed prompt verbatim), `recurring: true`.
2. Briefly confirm: what's scheduled, the cron expression, the human-readable cadence, that recurring tasks auto-expire after {MAX_AGE_DAYS} days, and that the user can cancel sooner with {CRON_DELETE} (include the job ID).{ypc}
3. **Then immediately execute the parsed prompt now** — don't wait for the first cron fire. If it's a slash command, invoke it via the Skill tool; otherwise act on it directly.",
        ypc = remote_confirm_line(),
    )
}

/// Binary `gZm(e)` (cc_all.txt:521920) — the DYNAMIC-flag prompt builder (selected
/// when `q_e()` is on and the input HAS a prompt). Splices the dynamic-mode
/// self-pacing block, the cron table, and the fixed-interval action. `${zpc()}`
/// (own line) renders "" → a preserved blank line before `## Fixed-interval mode`.
fn build_dynamic_prompt(args: &str) -> String {
    let dynamic_block = format!(
        "The user wants you to self-pace. Decide what makes the next iteration worth running — a passage of time, or an observable event.
1. **Run the parsed prompt now.** If it's a slash command, invoke it via the Skill tool; otherwise act on it directly.
2. **If the next run is gated on an event** (CI finishing, a log line matching, a file changing, a PR comment) and no {MONITOR} is already running for it: arm one now with `persistent: true`. Its events arrive as `<task-notification>` messages and wake this loop immediately — you do not wait for the {SCHEDULE_WAKEUP} deadline. Arm once; on later iterations call {TASK_LIST} first and skip this step if a monitor is already running.
3. **Briefly confirm**: that you're self-pacing, whether a {MONITOR} is the primary wake signal, that you ran the task now, and what fallback delay you're about to pick. Write this as text *before* calling {SCHEDULE_WAKEUP} — the turn ends as soon as that tool returns.
4. **Then, as the last action of this turn, call {SCHEDULE_WAKEUP}** with:
   - `delaySeconds`: with a {MONITOR} armed this is the **fallback heartbeat** — how long to wait if no event fires (lean 1200–1800s; idle ticks past the 5-minute cache window are pure overhead). Without a {MONITOR} this is the cadence — pick based on what you observed. Read the tool's own description for cache-aware delay guidance.
   - `reason`: one short sentence on why you picked that delay.
   - `prompt`: the full original /loop input verbatim, prefixed with `/loop ` so the next firing re-enters this skill and continues the loop. For example, if the user typed `/loop check the deploy`, pass `/loop check the deploy` as the prompt.
5. **If you were woken by a `<task-notification>`** rather than this prompt: handle the event in the context of the loop task, then call {SCHEDULE_WAKEUP} again with the same `prompt` and the same 1200–1800s `delaySeconds` from step 4 — the {MONITOR} remains the wake signal; this only resets the safety net.
6. **To stop the loop**, omit the {SCHEDULE_WAKEUP} call and {TASK_STOP} any {MONITOR} you armed (use {TASK_LIST} to find the task ID if it is no longer in context).{kpc}",
        kpc = push_outcome_line(),
    );
    format!(
        "# /loop — schedule a recurring or self-paced prompt
Parse the input below into `[interval] <prompt…>` and schedule it.
## Parsing (in priority order)
1. **Leading token**: if the first whitespace-delimited token matches `^\\d+[smhd]$` (e.g. `5m`, `2h`), that's the interval; the rest is the prompt.
2. **Trailing \"every\" clause**: otherwise, if the input ends with `every <N><unit>` or `every <N> <unit-word>` (e.g. `every 20m`, `every 5 minutes`, `every 2 hours`), extract that as the interval and strip it from the prompt. Only match when what follows \"every\" is a time expression — `check every PR` has no interval.
3. **No interval**: otherwise, the entire input is the prompt and you'll self-pace dynamically (see \"Dynamic mode\" below).
If the resulting prompt is empty, show usage `/loop [interval] <prompt>` and stop.
Examples:
- `5m /babysit-prs` → interval `5m`, prompt `/babysit-prs` (rule 1)
- `check the deploy every 20m` → interval `20m`, prompt `check the deploy` (rule 2)
- `run tests every 5 minutes` → interval `5m`, prompt `run tests` (rule 2)
- `check the deploy` → no interval → dynamic mode, prompt `check the deploy` (rule 3)
- `check every PR` → no interval → dynamic mode, prompt `check every PR` (rule 3 — \"every\" not followed by time)
- `5m` → empty prompt → show usage
{zpc}
## Fixed-interval mode (rules 1 and 2)
Convert the interval to a cron expression:
{table}
Then:
{action}
## Dynamic mode (rule 3 — no interval)
{dynamic}
## Input
{args}",
        zpc = cloud_offer_block(),
        table = CRON_TABLE,
        action = fixed_interval_action(),
        dynamic = dynamic_block,
    )
}

/// Binary `a(c,u)` (cc_all.txt:521920) — the NO-PROMPT autonomous-default builder.
/// `loop_file` = `c` (loop.md contents or `None`); `dynamic` = `u`; `interval` =
/// `i` (the parsed/normalized interval, used only on the cron path). When
/// `loop_file` is `None` it logs the activation telemetry and inlines the
/// autonomous-loop preamble; with a loop.md it inlines the file contents.
fn build_autonomous(loop_file: Option<&LoopFile>, dynamic: bool, interval: &str) -> String {
    // `d` — the inlined-instructions section header.
    let header = match loop_file {
        Some(f) => format!("## Loop tasks (from {})", f.path.display()),
        None => {
            "## Autonomous-loop instructions (for the immediate execution and every fire)".to_string()
        }
    };
    // `p` — the inlined instruction body. Absent loop.md → log activation +
    // preamble (binary `else QVe.logAutonomousLoopActivation(),p=…preamble`).
    let body = match loop_file {
        Some(f) => f.content.clone(),
        None => {
            log_autonomous_loop_activation();
            get_autonomous_loop_preamble().to_string()
        }
    };
    // `m` — the human label for "run X now".
    let what = if loop_file.is_some() {
        "the loop.md tasks"
    } else {
        "the autonomous check"
    };

    if dynamic {
        // `T` — the dynamic-mode sentinel to reschedule with.
        let sentinel = if loop_file.is_some() {
            LOOP_FILE_DYNAMIC_SENTINEL
        } else {
            AUTONOMOUS_LOOP_DYNAMIC_SENTINEL
        };
        // `y` — the heading + framing.
        let heading = match loop_file {
            Some(f) => format!(
                "# /loop — loop.md tasks with dynamic pacing
The user invoked `/loop` with no prompt and no interval and has a loop-tasks file at `{}`. Run those tasks now, then self-pace the next iteration via {SCHEDULE_WAKEUP} — no cron.",
                f.path.display()
            ),
            None => format!(
                "# /loop — autonomous default with dynamic pacing
The user invoked `/loop` with no prompt and no interval. Run the autonomous check now, then self-pace the next iteration via {SCHEDULE_WAKEUP} — no cron."
            ),
        };
        // `S` — the confirm-step phrasing.
        let confirm = match loop_file {
            Some(f) => format!(
                "that you're running tasks from `{}` in dynamic-pacing mode, that you ran the first tick now",
                f.path.display()
            ),
            None => {
                "that this is the autonomous default in dynamic-pacing mode, that you ran the check now".to_string()
            }
        };
        // `E` — the 6-step action block.
        let action = format!(
            "1. **Run {what} now**, following the instructions inlined below.
2. **If the next tick is gated on an event** (CI finishing, a PR comment, a log line) and no {MONITOR} is already running for it: arm one now with `persistent: true`. Its events wake this loop immediately — you do not wait for the {SCHEDULE_WAKEUP} deadline. Arm once; on later ticks call {TASK_LIST} first and skip if a monitor is already running.
3. **Briefly confirm**: {confirm}, whether a {MONITOR} is the primary wake signal, and what fallback delay you're about to pick. Write this as text *before* calling {SCHEDULE_WAKEUP} — the turn ends as soon as that tool returns.
4. **Then, as the last action of this turn, call {SCHEDULE_WAKEUP}** with:
   - `delaySeconds`: with a {MONITOR} armed this is the fallback heartbeat (lean 1200–1800s). Without one, pick based on what you observed this turn — quiet branch? wait longer. Lots in flight? wait shorter. Read the tool's own description for cache-aware delay guidance.
   - `reason`: one short sentence on why you picked that delay.
   - `prompt`: the literal string `{sentinel}` — the dynamic-mode sentinel expands at fire time to the full instructions (first fire / first fire post-compact / loop.md edited) or a dynamic-pacing-specific short reminder (subsequent fires). Do not pass the full instructions; that is handled automatically.
5. **If woken by a `<task-notification>`** rather than this prompt: handle the event, then call {SCHEDULE_WAKEUP} again with `{sentinel}` and the same 1200–1800s `delaySeconds` — the {MONITOR} remains the wake signal; this only resets the safety net.
6. **To stop the loop**, omit the {SCHEDULE_WAKEUP} call and {TASK_STOP} any {MONITOR} you armed (use {TASK_LIST} to find the task ID if it is no longer in context).{kpc}",
            kpc = push_outcome_line(),
        );
        return format!("{heading}\n## Action\n{action}\n{header}\n{body}");
    }

    // Cron path (`u` false).
    let sentinel = if loop_file.is_some() {
        LOOP_FILE_SENTINEL
    } else {
        AUTONOMOUS_LOOP_SENTINEL
    };
    let heading = match loop_file {
        Some(f) => format!(
            "# /loop — schedule loop.md tasks
The user invoked `/loop` with no prompt (input was empty or just the interval `{interval}`) and has a loop-tasks file at `{}`. Schedule a recurring cron that runs those tasks each tick, then run the first tick immediately.",
            f.path.display()
        ),
        None => format!(
            "# /loop — schedule the autonomous default
The user invoked `/loop` with no prompt (input was empty or just the interval `{interval}`). Schedule the autonomous-loop default and then run the first autonomous check immediately."
        ),
    };
    // `g` — the sentinel-expansion explainer.
    let expands = if loop_file.is_some() {
        "it expands at fire time to the full loop.md contents on first delivery (and whenever loop.md has been edited since last fire), and to a short reminder on subsequent unchanged fires. The long instructions stay in the cached message-prefix."
    } else {
        "it expands at fire time to the full autonomous-loop instructions on first delivery, and to a short reminder on subsequent fires (the long instructions stay in the cached message-prefix)."
    };
    // `_` — the confirm phrasing.
    let confirm = match loop_file {
        Some(f) => format!(
            "what's scheduled, the cron expression, the human-readable cadence, that it's running tasks from `{}`, that recurring tasks auto-expire after {MAX_AGE_DAYS} days, and that the user can cancel sooner with {CRON_DELETE} (include the job ID).",
            f.path.display()
        ),
        None => format!(
            "what's scheduled, the cron expression, the human-readable cadence, that recurring tasks auto-expire after {MAX_AGE_DAYS} days, and that they can cancel sooner with {CRON_DELETE} (include the job ID). Mention this is the autonomous default and that the autonomous-loop instructions are baked in."
        ),
    };
    format!(
        "{heading}
## Action
1. Convert `{interval}` to a 5-field cron expression. Supported suffixes: `s` → ceil to nearest minute, `m` (minutes), `h` (hours), `d` (days). Examples: `5m` → `*/5 * * * *`, `1h` → `0 * * * *`, `1d` → `0 0 * * *`. If the interval doesn't cleanly divide its unit, round to the nearest clean interval and tell the user what you rounded to.
2. Call {CRON_CREATE} with:
   - `cron`: the expression from step 1
   - `prompt`: the literal string `{sentinel}` — {expands}
   - `recurring`: `true`
3. Briefly confirm: {confirm}
4. **Then immediately run {what} now**, following the instructions inlined below. Don't wait for the first cron fire.
{header}
{body}"
    )
}

/// The `/loop` bundled-skill prompt builder (port of `getPromptForCommand`,
/// `_Zm`, cc_all.txt:521920). Two-flag dispatch matrix:
///
/// 1. **No-prompt** (empty or interval-only) **&& `isLoopDefaultPromptEnabled()`**
///    → the autonomous-default builder `a(loopFile, dynamic)`. `dynamic` is true
///    only when the input is fully empty AND `q_e()` is on; else cron.
/// 2. **`q_e()`** (dynamic flag) → `hZm()` usage (empty) / `gZm(n)` (prompt).
/// 3. **default** (both flags off, the SHIPPED binary) → `dZm` usage (empty) /
///    `fZm(n)` cron prompt.
///
/// With both `tengu_kairos_loop_prompt` and `tengu_kairos_loop_dynamic` at their
/// shipped default `false` (no live GrowthBook) only branch 3 is reachable —
/// byte-identical to the binary.
pub struct LoopPromptFn;

impl BundledPromptFn for LoopPromptFn {
    fn build(&self, args: &str) -> String {
        // Binary `let n=e.trim()` (cc_all.txt:521921).
        let n = args.trim();
        let every = every_clause_re().captures(n);
        let empty = n.is_empty();
        // `s` = lZm.test(n) || r!==null — input is just an interval.
        let interval_only = interval_only_re().is_match(n) || every.is_some();

        // Branch 1: no-prompt autonomous default (gated on isLoopDefaultPromptEnabled).
        if (empty || interval_only) && is_loop_default_prompt_enabled() {
            // `i = r ? uZm(r) : n || VSt`.
            let interval = if let Some(caps) = &every {
                normalize_every(&caps[1], &caps[2])
            } else if empty {
                DEFAULT_INTERVAL.to_string()
            } else {
                n.to_string()
            };
            let cwd = std::env::current_dir().unwrap_or_default();
            let loop_file = read_loop_file(&cwd);
            // `if(o&&q_e())return a(l,!0);return a(l,!1)` — dynamic only when empty.
            let dynamic = empty && is_loop_dynamic_enabled();
            return build_autonomous(loop_file.as_ref(), dynamic, &interval);
        }

        // Branch 2: dynamic-flag path (q_e on, but input has a prompt or the
        // default-prompt flag is off).
        if is_loop_dynamic_enabled() {
            if empty {
                // PARITY: `hZm()`.
                return USAGE_MESSAGE_DYNAMIC.to_string();
            }
            // PARITY: `gZm(n)`.
            return build_dynamic_prompt(n);
        }

        // Branch 3: shipped default — `!n ? dZm : fZm(n)`.
        if empty {
            USAGE_MESSAGE.to_string()
        } else {
            build_prompt(n)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_message_byte_exact() {
        // Asserts branch-3 (flag-off) default; serialize + clear flags.
        let _g = no_persistent_guard();
        // PARITY: binary dZm (cc_all.txt:521947) — empty/whitespace args → USAGE.
        let expected = "Usage: /loop [interval] <prompt>\nRun a prompt or slash command on a recurring interval.\nIntervals: Ns, Nm, Nh, Nd (e.g. 5m, 30m, 2h, 1d). Minimum granularity is 1 minute.\nIf no interval is specified, defaults to 10m.\nExamples:\n  /loop 5m /babysit-prs\n  /loop 30m check the deploy\n  /loop 1h /standup 1\n  /loop check the deploy          (defaults to 10m)\n  /loop check the deploy every 20m";
        assert_eq!(LoopPromptFn.build(""), expected);
        assert_eq!(LoopPromptFn.build("   "), expected);
        assert_eq!(LoopPromptFn.build("\n\t "), expected);
    }

    #[test]
    fn dynamic_usage_byte_exact() {
        // PARITY: binary hZm (cc_all.txt:521879-521889).
        let expected = "Usage: /loop [interval] <prompt>\nRun a prompt or slash command on a recurring interval — or with no interval, let the model self-pace based on the task.\nIntervals: Ns, Nm, Nh, Nd (e.g. 5m, 30m, 2h, 1d). Minimum granularity is 1 minute.\nIf no interval is specified, the model picks a delay between iterations based on what it's doing.\nExamples:\n  /loop 5m /babysit-prs\n  /loop 30m check the deploy\n  /loop 1h /standup 1\n  /loop check the deploy          (dynamic — model picks delays)\n  /loop check the deploy every 20m";
        assert_eq!(USAGE_MESSAGE_DYNAMIC, expected);
    }

    #[test]
    fn cron_head_byte_exact() {
        // PARITY: binary fZm head (cc_all.txt:521800-521846) — single newlines
        // except the blank line before `## Interval` from the empty `${zpc()}` on
        // its own line; ${default}=10m, tool names baked in, zpc()/Ypc() empty.
        let expected = "# /loop — schedule a recurring prompt\nParse the input below into `[interval] <prompt…>` and schedule it with CronCreate.\n## Parsing (in priority order)\n1. **Leading token**: if the first whitespace-delimited token matches `^\\d+[smhd]$` (e.g. `5m`, `2h`), that's the interval; the rest is the prompt.\n2. **Trailing \"every\" clause**: otherwise, if the input ends with `every <N><unit>` or `every <N> <unit-word>` (e.g. `every 20m`, `every 5 minutes`, `every 2 hours`), extract that as the interval and strip it from the prompt. Only match when what follows \"every\" is a time expression — `check every PR` has no interval.\n3. **Default**: otherwise, interval is `10m` and the entire input is the prompt.\nIf the resulting prompt is empty, show usage `/loop [interval] <prompt>` and stop — do not call CronCreate.\nExamples:\n- `5m /babysit-prs` → interval `5m`, prompt `/babysit-prs` (rule 1)\n- `check the deploy every 20m` → interval `20m`, prompt `check the deploy` (rule 2)\n- `run tests every 5 minutes` → interval `5m`, prompt `run tests` (rule 2)\n- `check the deploy` → interval `10m`, prompt `check the deploy` (rule 3)\n- `check every PR` → interval `10m`, prompt `check every PR` (rule 3 — \"every\" not followed by time)\n- `5m` → empty prompt → show usage\n\n## Interval → cron\nSupported suffixes: `s` (seconds, rounded up to nearest minute, min 1), `m` (minutes), `h` (hours), `d` (days). Convert:\n| Interval pattern      | Cron expression     | Notes                                    |\n|-----------------------|---------------------|------------------------------------------|\n| `Nm` where N ≤ 59   | `*/N * * * *`     | every N minutes                          |\n| `Nm` where N ≥ 60   | `0 */H * * *`     | round to hours (H = N/60, must divide 24)|\n| `Nh` where N ≤ 23   | `0 */N * * *`     | every N hours                            |\n| `Nd`                | `0 0 */N * *`     | every N days at midnight local           |\n| `Ns`                | treat as `ceil(N/60)m` | cron minimum granularity is 1 minute  |\n**If the interval doesn't cleanly divide its unit** (e.g. `7m` → `*/7 * * * *` gives uneven gaps at :56→:00; `90m` → 1.5h which cron can't express), pick the nearest clean interval and tell the user what you rounded to before scheduling.\n## Action\n1. Call CronCreate with:\n   - `cron`: the expression from the table above\n   - `prompt`: the parsed prompt from above, verbatim (slash commands are passed through unchanged)\n   - `recurring`: `true`\n2. Briefly confirm: what's scheduled, the cron expression, the human-readable cadence, that recurring tasks auto-expire after 30 days, and that they can cancel sooner with CronDelete (include the job ID).\n3. **Then immediately execute the parsed prompt now** — don't wait for the first cron fire. If it's a slash command, invoke it via the Skill tool; otherwise act on it directly.";
        assert_eq!(cron_prompt_head(), expected);
    }

    #[test]
    fn build_prompt_structure() {
        let _g = no_persistent_guard();
        // PARITY: binary fZm = head + `\n## Input\n${e}` (cc_all.txt:521846).
        let out = LoopPromptFn.build("5m /babysit-prs");
        assert!(out.starts_with(&cron_prompt_head()));
        assert!(out.ends_with("\n## Input\n5m /babysit-prs"));
        // No fabricated dynamic section is spliced in.
        let action = out.find("## Action").unwrap();
        let input = out.find("## Input").unwrap();
        assert!(action < input);
    }

    #[test]
    fn no_fabricated_dynamic_section() {
        let _g = no_persistent_guard();
        // The previous SYNTHESIZED `## Self-pace (dynamic) mode` addendum (which
        // existed NOWHERE in the binary) is gone.
        let out = LoopPromptFn.build("keep working on the migration");
        assert!(!out.contains("## Self-pace"));
        assert!(!out.contains("## Self-pace (dynamic) mode"));
    }

    #[test]
    fn build_prompt_trims_and_interpolates_args() {
        let _g = no_persistent_guard();
        // Binary `n=e.trim()` then `fZm(n)` — trimmed text appears verbatim under
        // `## Input`.
        let out = LoopPromptFn.build("  check the deploy  ");
        assert!(out.ends_with("\n## Input\ncheck the deploy"));
        assert!(out.contains("schedule it with CronCreate."));
        assert!(out.contains("cancel sooner with CronDelete"));
        assert!(out.contains("auto-expire after 30 days"));
    }

    // ── Flag-on (autonomous-default / dynamic-pacing) builders ───────────────
    //
    // These assert byte-exactness against fixtures reconstructed INDEPENDENTLY in
    // Python directly from the 2.1.191 binary's template literals (see
    // `tests/fixtures/loop_autonomous/`). The fixtures are a second transcription
    // of the binary; agreement with the Rust impl == byte-parity. The `None`
    // (autonomous, no loop.md) cases inline the default preamble `aJr` — assert it
    // ends with the binary preamble (kept apart so a preamble edit doesn't have to
    // re-touch every fixture).

    /// Serialize the env-var + preamble-dependent builder tests within this crate.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn no_persistent_guard() -> std::sync::MutexGuard<'static, ()> {
        let g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        // Establish the shipped-binary default: persistent off (preamble = aJr),
        // and the prompt/dynamic flags cleared. Gates now read the flag override
        // layer (binary `nt`), so reset it rather than env vars.
        std::env::remove_var("LINGXI_LOOP_PERSISTENT");
        telemetry::test_clear_flag("tengu_kairos_loop_persistent");
        telemetry::test_clear_flag("tengu_kairos_loop_prompt");
        telemetry::test_clear_flag("tengu_kairos_loop_dynamic");
        g
    }

    fn fixture(name: &str) -> String {
        match name {
            "a_auto_cron" => {
                include_str!("../../tests/fixtures/loop_autonomous/a_auto_cron.txt").to_string()
            }
            "a_auto_dynamic" => {
                include_str!("../../tests/fixtures/loop_autonomous/a_auto_dynamic.txt").to_string()
            }
            "a_loopfile_cron" => {
                include_str!("../../tests/fixtures/loop_autonomous/a_loopfile_cron.txt").to_string()
            }
            "a_loopfile_dynamic" => {
                include_str!("../../tests/fixtures/loop_autonomous/a_loopfile_dynamic.txt")
                    .to_string()
            }
            "gZm" => include_str!("../../tests/fixtures/loop_autonomous/gZm.txt").to_string(),
            _ => unreachable!(),
        }
    }

    #[test]
    fn autonomous_cron_builder_byte_exact() {
        let _g = no_persistent_guard();
        // a(null, u=false, i="10m") — the autonomous-default cron prompt.
        assert_eq!(build_autonomous(None, false, "10m"), fixture("a_auto_cron"));
        // Sanity: the inlined body IS the binary default preamble (aJr).
        assert!(build_autonomous(None, false, "10m")
            .ends_with(tool_cron::AUTONOMOUS_LOOP_PREAMBLE));
    }

    #[test]
    fn autonomous_dynamic_builder_byte_exact() {
        let _g = no_persistent_guard();
        // a(null, u=true, i unused) — the autonomous-default dynamic-pacing prompt.
        assert_eq!(build_autonomous(None, true, "10m"), fixture("a_auto_dynamic"));
        assert!(build_autonomous(None, true, "10m")
            .ends_with(tool_cron::AUTONOMOUS_LOOP_PREAMBLE));
    }

    #[test]
    fn loopfile_cron_builder_byte_exact() {
        let _g = no_persistent_guard();
        let lf = LoopFile {
            path: std::path::PathBuf::from("/tmp/proj/loop.md"),
            content: "- task A\n- task B".to_string(),
        };
        assert_eq!(
            build_autonomous(Some(&lf), false, "5m"),
            fixture("a_loopfile_cron")
        );
    }

    #[test]
    fn loopfile_dynamic_builder_byte_exact() {
        let _g = no_persistent_guard();
        let lf = LoopFile {
            path: std::path::PathBuf::from("/tmp/proj/loop.md"),
            content: "- task A\n- task B".to_string(),
        };
        assert_eq!(
            build_autonomous(Some(&lf), true, "10m"),
            fixture("a_loopfile_dynamic")
        );
    }

    #[test]
    fn dynamic_prompt_builder_byte_exact() {
        // gZm("check the deploy") — the dynamic-flag prompt builder.
        assert_eq!(build_dynamic_prompt("check the deploy"), fixture("gZm"));
    }

    #[test]
    fn normalize_every_matches_uzm() {
        // Binary uZm: s→Ns, h→Nh, d→Nd, else Nm.
        assert_eq!(normalize_every("5", "minutes"), "5m");
        assert_eq!(normalize_every("2", "hours"), "2h");
        assert_eq!(normalize_every("30", "secs"), "30s");
        assert_eq!(normalize_every("1", "day"), "1d");
        assert_eq!(normalize_every("20", "m"), "20m");
    }

    #[test]
    fn dispatch_default_off_is_shipped_binary() {
        let _g = no_persistent_guard();
        // guard() clears the prompt/dynamic flags → branch 3 only.
        // Empty → dZm usage, prompt → fZm cron.
        assert_eq!(LoopPromptFn.build(""), USAGE_MESSAGE);
        assert!(LoopPromptFn.build("5m /foo").starts_with("# /loop — schedule a recurring prompt"));
        // No autonomous/dynamic headers leak in.
        assert!(!LoopPromptFn.build("").contains("autonomous default"));
    }

    #[test]
    fn dispatch_flag_on_no_prompt_routes_to_autonomous() {
        let _g = no_persistent_guard();
        telemetry::test_set_flag("tengu_kairos_loop_prompt", true);
        // Empty input + flag on → autonomous default. cwd has no loop.md in the
        // test sandbox → the autonomous (None) cron builder.
        let out = LoopPromptFn.build("");
        assert!(out.starts_with("# /loop — schedule the autonomous default"));
        // Interval-only stays cron even with the dynamic flag on (binary: dynamic
        // only when input is fully empty).
        telemetry::test_set_flag("tengu_kairos_loop_dynamic", true);
        let interval_only = LoopPromptFn.build("5m");
        assert!(interval_only.starts_with("# /loop — schedule the autonomous default"));
        // Fully empty + both flags → dynamic-pacing autonomous default.
        let empty_dyn = LoopPromptFn.build("");
        assert!(empty_dyn.starts_with("# /loop — autonomous default with dynamic pacing"));
        telemetry::test_clear_flag("tengu_kairos_loop_prompt");
        telemetry::test_clear_flag("tengu_kairos_loop_dynamic");
    }

    #[test]
    fn dispatch_dynamic_flag_with_prompt_uses_gzm() {
        let _g = no_persistent_guard();
        telemetry::test_set_flag("tengu_kairos_loop_dynamic", true);
        // q_e on, prompt present, default-prompt flag off → gZm.
        let out = LoopPromptFn.build("check the deploy");
        assert!(out.starts_with("# /loop — schedule a recurring or self-paced prompt"));
        assert!(out.ends_with("\n## Input\ncheck the deploy"));
        // Empty → hZm dynamic usage.
        assert_eq!(LoopPromptFn.build(""), USAGE_MESSAGE_DYNAMIC);
        telemetry::test_clear_flag("tengu_kairos_loop_dynamic");
    }
}
