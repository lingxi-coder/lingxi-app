//! Bundled `/loop` skill — 1:1 port of the 2.1.191 binary's `/loop`
//! `getPromptForCommand` builder.
//!
//! The binary registers `/loop` via `_Zm()` (cc_all.txt:521920) with a
//! `getPromptForCommand(e,t)` that dispatches on TWO feature flags:
//!   - `q_e()` = `tengu_kairos_loop_dynamic` (default **false**)
//!   - `isLoopDefaultPromptEnabled()` = `tengu_kairos_loop_prompt` (default false)
//! With both flags off (the SHIPPED default), the dispatch is:
//!   - empty/interval-only input → `dZm` (the USAGE message)
//!   - otherwise               → `fZm(input)` (the cron-mode head + `## Input`)
//! (cc_all.txt:521946 — `if(!n)return…dZm;return…fZm(n)`).
//!
//! The port has NO backend for the `tengu_kairos_loop_*` flags (the `features`
//! crate lacks those keys), so it implements the default (cron) path, which is
//! exactly what the shipped binary returns. The dynamic-enabled builders
//! (`hZm`/`gZm` + the no-prompt autonomous builder) are gated on those default-
//! off flags and need loop.md / remote-session infra the port lacks; see the
//! PARITY-TODO on [`USAGE_MESSAGE_DYNAMIC`].
//!
//! Literal interpolations resolve to constants here: `${VSt}` = `10m`,
//! `${xw}` = `CronCreate`, `${t9}` = `CronDelete`, `${lte}` = `30`
//! (`DEFAULT_MAX_AGE_DAYS`).

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

/// Binary `hZm` (cc_all.txt:521879) — the USAGE message for the DYNAMIC variant
/// (returned only when `q_e()` = `tengu_kairos_loop_dynamic` is enabled).
// PARITY: binary hZm (cc_all.txt:521879-521889).
// PARITY-TODO: the dynamic dispatch path (hZm usage + gZm builder + the
// no-prompt autonomous builder) is gated on `tengu_kairos_loop_dynamic` /
// `tengu_kairos_loop_prompt`, both DEFAULT FALSE and ABSENT from the port's
// `features` crate (no flag backend), and needs the loop.md / autonomous-
// preamble infra. The shipped binary default returns the cron variants, which
// the port implements. This const exists so the usage surface can switch when a
// flag backend is wired; the gZm/autonomous builders remain TODO.
#[allow(dead_code)] // PARITY const; selected only when the dynamic flag is wired.
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

/// The `/loop` bundled-skill prompt builder (port of `getPromptForCommand`,
/// `_Zm`, cc_all.txt:521920): empty (or whitespace-only) args → USAGE; else →
/// `buildPrompt(trimmed)`.
///
/// PARITY: with `q_e()` / `isLoopDefaultPromptEnabled()` both default-false, the
/// binary dispatch is `!n?dZm:fZm(n)` (cc_all.txt:521946) — the cron variants.
pub struct LoopPromptFn;

impl BundledPromptFn for LoopPromptFn {
    fn build(&self, args: &str) -> String {
        // Binary `let n=e.trim()` (cc_all.txt:521921).
        let trimmed = args.trim();
        if trimmed.is_empty() {
            // PARITY: `!n` → `dZm` (cron usage), the shipped default.
            USAGE_MESSAGE.to_string()
        } else {
            // PARITY: `fZm(n)` (cron head), the shipped default.
            build_prompt(trimmed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_message_byte_exact() {
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
        // The previous SYNTHESIZED `## Self-pace (dynamic) mode` addendum (which
        // existed NOWHERE in the binary) is gone.
        let out = LoopPromptFn.build("keep working on the migration");
        assert!(!out.contains("## Self-pace"));
        assert!(!out.contains("## Self-pace (dynamic) mode"));
    }

    #[test]
    fn build_prompt_trims_and_interpolates_args() {
        // Binary `n=e.trim()` then `fZm(n)` — trimmed text appears verbatim under
        // `## Input`.
        let out = LoopPromptFn.build("  check the deploy  ");
        assert!(out.ends_with("\n## Input\ncheck the deploy"));
        assert!(out.contains("schedule it with CronCreate."));
        assert!(out.contains("cancel sooner with CronDelete"));
        assert!(out.contains("auto-expire after 30 days"));
    }
}
