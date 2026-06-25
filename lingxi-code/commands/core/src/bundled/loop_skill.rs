//! Bundled `/loop` skill — 1:1 port of
//! `claude-code/src/skills/bundled/loop.ts`.
//!
//! The reference registers a programmatic bundled skill whose
//! `getPromptForCommand(args)` returns the USAGE message for empty args and the
//! `buildPrompt(trimmed)` text otherwise (loop.ts:84-90). The port carries this
//! two-branch behavior through [`command_api::BundledPromptFn`] (see
//! [`LoopPromptFn`]), invoked by the `Skill` tool at call time.
//!
//! All literal interpolations from loop.ts resolve to constants here:
//! `${DEFAULT_INTERVAL}` = `10m` (loop.ts:9), `${CRON_CREATE_TOOL_NAME}` =
//! `CronCreate`, `${CRON_DELETE_TOOL_NAME}` = `CronDelete`,
//! `${DEFAULT_MAX_AGE_DAYS}` = `30` (`tools/cron`).

use command_api::BundledPromptFn;

/// loop.ts:9 — default interval when none is parsed from the input.
const DEFAULT_INTERVAL: &str = "10m";

/// loop.ts:11-23 — returned verbatim when the trimmed args are empty.
/// `${DEFAULT_INTERVAL}` substituted to `10m`. No trailing newline (matches the
/// reference template literal, which ends at the last example line).
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

/// loop.ts:25-67 — the cron-mode head of `buildPrompt(args)`, everything through
/// the `## Action` block (BYTE-FAITHFUL to the reference, modulo the literal
/// interpolations `CronCreate` / `CronDelete` / `30` / `{default}` = `10m`).
/// Kept as its own const so the Phase-1 cron text stays byte-locked when the
/// SYNTHESIZED Phase-2 dynamic addendum (below) is spliced in before `## Input`.
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

/// Phase-2 dynamic / self-pace addendum (SYNTHESIZED — there is NO byte-faithful
/// reference; the leaked `loop.ts` predates this mode). Splices in between the
/// cron `## Action` block and the `## Input` section. Guides the model to the
/// `ScheduleWakeup` self-pace path when the user omitted the interval, derived
/// from the live ScheduleWakeup contract (spec `loop-impl-spec.md:132-179`).
const DYNAMIC_ADDENDUM: &str = "\n\n## Self-pace (dynamic) mode\n\nThe rules above are for a FIXED recurring interval. If the input has no interval AND the user asked you to keep working at a pace YOU choose (\"keep going until done\", \"work on this and check back when it makes sense\"), do NOT call CronCreate. Instead, after making progress this turn, call `ScheduleWakeup` to wake yourself up later and continue:\n\n- `delaySeconds`: seconds until the next iteration. The runtime clamps to [60,3600]. The Anthropic prompt cache has a 5-minute TTL: delaySeconds < 300 keeps the cache warm; 300-3600 pays a cache miss; avoid exactly 300. A typical idle tick is 1200-1800.\n- `reason`: one specific sentence explaining the chosen delay (surfaced to telemetry and the user).\n- `prompt`: the SAME /loop input verbatim each turn so the next firing repeats the task. For an autonomous /loop with no user prompt, pass the literal sentinel `<<autonomous-loop-dynamic>>`.\n\nTo END the loop, simply OMIT the `ScheduleWakeup` call — no further wake-up is scheduled.";

/// loop.ts:25-72 — `buildPrompt(args)` with `${args}` = the trimmed input.
/// The cron-mode head ([`cron_prompt_head`]) is byte-faithful to the reference;
/// the SYNTHESIZED [`DYNAMIC_ADDENDUM`] (Phase-2 self-pace mode) is spliced in
/// before the final `## Input` section, which ends with `\n\n${args}` (no
/// trailing newline, reproduced exactly).
fn build_prompt(args: &str) -> String {
    format!(
        "{head}{addendum}\n\n## Input\n\n{args}",
        head = cron_prompt_head(),
        addendum = DYNAMIC_ADDENDUM,
        args = args,
    )
}

/// The `/loop` bundled-skill prompt builder (port of `getPromptForCommand`,
/// loop.ts:84-90): empty (or whitespace-only) args → USAGE; else →
/// `buildPrompt(trimmed)`.
pub struct LoopPromptFn;

impl BundledPromptFn for LoopPromptFn {
    fn build(&self, args: &str) -> String {
        // loop.ts:85 — `const trimmed = args.trim()`.
        let trimmed = args.trim();
        if trimmed.is_empty() {
            // loop.ts:86-88.
            USAGE_MESSAGE.to_string()
        } else {
            // loop.ts:89.
            build_prompt(trimmed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_message_byte_exact() {
        // loop.ts:86-88 — empty (and whitespace-only) trimmed args → USAGE.
        let expected = "Usage: /loop [interval] <prompt>\n\nRun a prompt or slash command on a recurring interval.\n\nIntervals: Ns, Nm, Nh, Nd (e.g. 5m, 30m, 2h, 1d). Minimum granularity is 1 minute.\nIf no interval is specified, defaults to 10m.\n\nExamples:\n  /loop 5m /babysit-prs\n  /loop 30m check the deploy\n  /loop 1h /standup 1\n  /loop check the deploy          (defaults to 10m)\n  /loop check the deploy every 20m";
        assert_eq!(LoopPromptFn.build(""), expected);
        assert_eq!(LoopPromptFn.build("   "), expected);
        assert_eq!(LoopPromptFn.build("\n\t "), expected);
    }

    #[test]
    fn cron_head_byte_exact() {
        // loop.ts:25-67 (cron-mode head, through `## Action`) with ${default}=10m
        // and tool names baked in — the BYTE-LOCKED Phase-1 cron text.
        let expected = "# /loop — schedule a recurring prompt\n\nParse the input below into `[interval] <prompt…>` and schedule it with CronCreate.\n\n## Parsing (in priority order)\n\n1. **Leading token**: if the first whitespace-delimited token matches `^\\d+[smhd]$` (e.g. `5m`, `2h`), that's the interval; the rest is the prompt.\n2. **Trailing \"every\" clause**: otherwise, if the input ends with `every <N><unit>` or `every <N> <unit-word>` (e.g. `every 20m`, `every 5 minutes`, `every 2 hours`), extract that as the interval and strip it from the prompt. Only match when what follows \"every\" is a time expression — `check every PR` has no interval.\n3. **Default**: otherwise, interval is `10m` and the entire input is the prompt.\n\nIf the resulting prompt is empty, show usage `/loop [interval] <prompt>` and stop — do not call CronCreate.\n\nExamples:\n- `5m /babysit-prs` → interval `5m`, prompt `/babysit-prs` (rule 1)\n- `check the deploy every 20m` → interval `20m`, prompt `check the deploy` (rule 2)\n- `run tests every 5 minutes` → interval `5m`, prompt `run tests` (rule 2)\n- `check the deploy` → interval `10m`, prompt `check the deploy` (rule 3)\n- `check every PR` → interval `10m`, prompt `check every PR` (rule 3 — \"every\" not followed by time)\n- `5m` → empty prompt → show usage\n\n## Interval → cron\n\nSupported suffixes: `s` (seconds, rounded up to nearest minute, min 1), `m` (minutes), `h` (hours), `d` (days). Convert:\n\n| Interval pattern      | Cron expression     | Notes                                    |\n|-----------------------|---------------------|------------------------------------------|\n| `Nm` where N ≤ 59   | `*/N * * * *`     | every N minutes                          |\n| `Nm` where N ≥ 60   | `0 */H * * *`     | round to hours (H = N/60, must divide 24)|\n| `Nh` where N ≤ 23   | `0 */N * * *`     | every N hours                            |\n| `Nd`                | `0 0 */N * *`     | every N days at midnight local           |\n| `Ns`                | treat as `ceil(N/60)m` | cron minimum granularity is 1 minute  |\n\n**If the interval doesn't cleanly divide its unit** (e.g. `7m` → `*/7 * * * *` gives uneven gaps at :56→:00; `90m` → 1.5h which cron can't express), pick the nearest clean interval and tell the user what you rounded to before scheduling.\n\n## Action\n\n1. Call CronCreate with:\n   - `cron`: the expression from the table above\n   - `prompt`: the parsed prompt from above, verbatim (slash commands are passed through unchanged)\n   - `recurring`: `true`\n2. Briefly confirm: what's scheduled, the cron expression, the human-readable cadence, that recurring tasks auto-expire after 30 days, and that they can cancel sooner with CronDelete (include the job ID).\n3. **Then immediately execute the parsed prompt now** — don't wait for the first cron fire. If it's a slash command, invoke it via the Skill tool; otherwise act on it directly.";
        assert_eq!(cron_prompt_head(), expected);
    }

    #[test]
    fn build_prompt_structure() {
        // The full prompt = cron head + dynamic addendum + `## Input\n\n${args}`.
        let out = LoopPromptFn.build("5m /babysit-prs");
        assert!(out.starts_with(&cron_prompt_head()));
        assert!(out.ends_with("## Input\n\n5m /babysit-prs"));
        // The cron `## Action` block precedes the dynamic `## Self-pace` section,
        // which precedes `## Input`.
        let action = out.find("## Action").unwrap();
        let selfpace = out.find("## Self-pace (dynamic) mode").unwrap();
        let input = out.find("## Input").unwrap();
        assert!(action < selfpace && selfpace < input);
    }

    #[test]
    fn dynamic_addendum_present() {
        // Phase-2 self-pace mode (SYNTHESIZED) is reachable from buildPrompt.
        let out = LoopPromptFn.build("keep working on the migration");
        assert!(out.contains("## Self-pace (dynamic) mode"));
        assert!(out.contains("ScheduleWakeup"));
        assert!(out.contains("<<autonomous-loop-dynamic>>"));
        assert!(out.contains("clamps to [60,3600]"));
        assert!(out.contains("OMIT the `ScheduleWakeup` call"));
    }

    #[test]
    fn build_prompt_trims_and_interpolates_args() {
        // loop.ts:85,89 — `args.trim()` then `buildPrompt(trimmed)`; the trimmed
        // text appears verbatim under `## Input`.
        let out = LoopPromptFn.build("  check the deploy  ");
        assert!(out.ends_with("## Input\n\ncheck the deploy"));
        // The cron tool names are baked in (not placeholders).
        assert!(out.contains("schedule it with CronCreate."));
        assert!(out.contains("cancel sooner with CronDelete"));
        assert!(out.contains("auto-expire after 30 days"));
    }
}
