//! `CronCreateTool` — schedule a prompt on a 5-field cron schedule and persist
//! the job into the project's single `<root>/.claude/scheduled_tasks.json` file.
//!
//! 1:1 parity rewrite of claude-code `CronCreateTool.ts`. The model supplies a
//! 5-field cron expression plus the prompt to enqueue at each fire time, with
//! optional `recurring` (default true) and `durable` (default false) flags.
//!
//! Wire identifiers:
//! - 5-field cron expressions only (6-field with seconds is rejected by the
//!   shared parser and surfaces as the generic "Expected 5 fields" message).
//! - `id = [d][0-9a-z]{8}` (9-char format).
//!
//! Persistence (1:1 with claude-code `cronTasks.ts`): a DURABLE job is appended
//! to the single project-relative file `<projectRoot>/.claude/scheduled_tasks.json`
//! shaped `{ "tasks": [ CronTask, … ] }` (camelCase fields, `createdAt` in epoch
//! **milliseconds**, NO `durable`/next-fire on disk — those are runtime-only).
//! A SESSION-ONLY job (`durable:false`) is NOT written to disk at all (matching
//! claude-code's separate in-memory session store and the tool's own
//! "Session-only (not written to disk …)" promise).
//!
//! This seam does NOT register the job into a live scheduler — it parses,
//! validates, and persists. Production hosts pick the file up via
//! [`cron::scheduler::CronScheduler`], which computes each job's next fire time
//! at runtime from the cron string + `lastFiredAt ?? createdAt`.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use async_trait::async_trait;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Value};
use telemetry::pii::{PiiTagged, Verified};
use telemetry::sink::{AnalyticsValue, LogEventMetadata};
use telemetry::tengu::tool::{
    SCHEDULE_CRON_COMPLETED, SCHEDULE_CRON_FAILED, SCHEDULE_CRON_STARTED,
};
use telemetry::AnalyticsBus;

use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext, ValidationError,
};

// -- Inlined 5-field cron parser (mirrors cron::schedule) -------------
//
// We mirror the M1.17 cron::schedule::{parse_cron, CronExpression,
// CronField, decompose} surface inline because the natural dependency
// `lingxi-tools → lingxi-cron` would form a workspace cycle through
// `lingxi-cron → lingxi-tasks → lingxi-agent → lingxi-memory →
//  lingxi-sidequery → lingxi-tools`. The parser and matcher are byte-for-byte
// equivalent; downstream parity is verified by the (out-of-cycle) M4-09
// fixture driver against the canonical lingxi_cron table.

/// A parsed cron field: the expanded, sorted, de-duplicated set of integer
/// values it matches. Mirrors the `number[]` that `expandField` returns in
/// claude-code `utils/cron.ts`.
#[derive(Debug, Clone)]
struct CronField(Vec<u32>);

/// Inclusive `[min, max]` value range for a cron field. Mirrors the
/// `FieldRange` rows of the `FIELD_RANGES` table in `utils/cron.ts`.
#[derive(Debug, Clone, Copy)]
struct FieldRange {
    min: u32,
    max: u32,
}

/// Per-field value ranges, indexed minute, hour, day-of-month, month,
/// day-of-week — 1:1 with `cron.ts` `FIELD_RANGES`. Day-of-week is `0..=6`
/// (Sunday = 0) with `7` accepted as a Sunday alias and normalized to `0`.
const FIELD_RANGES: [FieldRange; 5] = [
    FieldRange { min: 0, max: 59 }, // minute
    FieldRange { min: 0, max: 23 }, // hour
    FieldRange { min: 1, max: 31 }, // day-of-month
    FieldRange { min: 1, max: 12 }, // month
    FieldRange { min: 0, max: 6 },  // day-of-week (0=Sunday; 7 = Sunday alias)
];

#[derive(Debug, Clone)]
struct CronExpression {
    minute: CronField,
    hour: CronField,
    dom: CronField,
    month: CronField,
    dow: CronField,
}

/// Expand one cron field into its sorted, de-duplicated set of matching values.
///
/// 1:1 port of `expandField` in claude-code `utils/cron.ts`. Each comma part is
/// independently one of: a wildcard / step (`*`, `*/N`), a range or stepped
/// range (`N-M`, `N-M/S`), or a single value (`N`). Mixed forms compose freely
/// in one field (e.g. `1-5,0`, `0-30/10,45`, `*/15,7`). Day-of-week `7` is
/// normalized to `0` (Sunday) — both as a bare value and at the high end of a
/// range. Any value outside the field's `[min, max]` range (with `7` allowed
/// for day-of-week) makes the whole field invalid. Returns `Err` — which maps
/// to TS `null` — on any unsupported or out-of-range form, so the caller emits
/// the generic "Invalid cron expression" message.
fn expand_field(field: &str, range: FieldRange) -> Result<CronField, String> {
    let FieldRange { min, max } = range;
    // Day-of-week is the only `0..=6` field; it accepts `7` as a Sunday alias.
    let is_dow = min == 0 && max == 6;
    let mut out: BTreeSet<u32> = BTreeSet::new();

    for part in field.split(',') {
        // wildcard or `*/N`  (regex `^\*(?:\/(\d+))?$`)
        if let Some(step) = match_star(part) {
            if step < 1 {
                return Err(format!("bad field: {field}"));
            }
            let mut i = min;
            while i <= max {
                out.insert(i);
                i = i.saturating_add(step);
            }
            continue;
        }

        // `N-M` or `N-M/S`  (regex `^(\d+)-(\d+)(?:\/(\d+))?$`)
        if let Some((lo, hi, step)) = match_range(part) {
            let eff_max = if is_dow { 7 } else { max };
            if lo > hi || step < 1 || lo < min || hi > eff_max {
                return Err(format!("bad field: {field}"));
            }
            let mut i = lo;
            while i <= hi {
                out.insert(if is_dow && i == 7 { 0 } else { i });
                i = i.saturating_add(step);
            }
            continue;
        }

        // plain `N`  (regex `^\d+$`)
        if let Some(mut n) = match_single(part) {
            if is_dow && n == 7 {
                n = 0;
            }
            if n < min || n > max {
                return Err(format!("bad field: {field}"));
            }
            out.insert(n);
            continue;
        }

        return Err(format!("bad field: {field}"));
    }

    if out.is_empty() {
        return Err(format!("bad field: {field}"));
    }
    Ok(CronField(out.into_iter().collect()))
}

/// Match `*` or `*/N`, returning the step (`1` for a bare `*`). Mirrors the
/// `^\*(?:\/(\d+))?$` branch of `expandField`.
fn match_star(part: &str) -> Option<u32> {
    if part == "*" {
        return Some(1);
    }
    let rest = part.strip_prefix("*/")?;
    if !all_digits(rest) {
        return None;
    }
    rest.parse::<u32>().ok()
}

/// Match `N-M` or `N-M/S`, returning `(lo, hi, step)` with `step` defaulting to
/// `1`. Mirrors the `^(\d+)-(\d+)(?:\/(\d+))?$` branch of `expandField`.
fn match_range(part: &str) -> Option<(u32, u32, u32)> {
    let (range_part, step) = match part.split_once('/') {
        Some((r, s)) => {
            if !all_digits(s) {
                return None;
            }
            (r, s.parse::<u32>().ok()?)
        }
        None => (part, 1),
    };
    let (lo, hi) = range_part.split_once('-')?;
    if !all_digits(lo) || !all_digits(hi) {
        return None;
    }
    Some((lo.parse::<u32>().ok()?, hi.parse::<u32>().ok()?, step))
}

/// Match a bare `N` (regex `^\d+$`).
fn match_single(part: &str) -> Option<u32> {
    if all_digits(part) {
        part.parse::<u32>().ok()
    } else {
        None
    }
}

fn parse_cron(s: &str) -> Result<CronExpression, String> {
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() != 5 {
        return Err(format!("expected 5 fields, got {}", parts.len()));
    }
    Ok(CronExpression {
        minute: expand_field(parts[0], FIELD_RANGES[0])?,
        hour: expand_field(parts[1], FIELD_RANGES[1])?,
        dom: expand_field(parts[2], FIELD_RANGES[2])?,
        month: expand_field(parts[3], FIELD_RANGES[3])?,
        dow: expand_field(parts[4], FIELD_RANGES[4])?,
    })
}

fn field_match(field: &CronField, value: u32) -> bool {
    field.0.contains(&value)
}

/// Decompose unix seconds into (year, month 1-12, day 1-31, hour, minute,
/// dow 0-6 Sun=0), UTC, on the real proleptic Gregorian calendar. Mirrors
/// `cron::schedule::decompose` — Howard Hinnant's integer-only `civil_from_days`,
/// so real month lengths (incl. leap-year Feb 29 and 31-day months) are honored.
/// This fixes the old fixed-30-day approximation under which day-of-month 31
/// could never match. Pure UTC core — [`CronExpression::matches`] shifts the
/// instant by [`local_offset_seconds`] first, so the `next_fire_unix_secs`
/// preview is evaluated in LOCAL time, consistent with the live `cron`
/// scheduler's local firing (claude-code `cron.ts` is local).
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]
fn decompose(secs: u64) -> (u32, u32, u32, u32, u32, u32) {
    let minute = (secs / 60 % 60) as u32;
    let hour = (secs / 3600 % 24) as u32;
    let days = (secs / 86_400) as i64; // days since 1970-01-01 (a Thursday)
    let dow = ((days % 7 + 4) % 7) as u32; // 1970-01-01 = Thursday (4)

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let year = (y + i64::from(month <= 2)) as u32;

    (year, month, day, hour, minute, dow)
}

impl CronExpression {
    fn matches(&self, time: SystemTime) -> bool {
        let secs = time
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        // LOCAL-time evaluation (claude-code parity + consistency with the live
        // `cron` scheduler): shift by the system UTC offset, then decompose.
        #[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
        let local = (secs as i64)
            .saturating_add(cron::schedule::local_offset_seconds(secs as i64))
            .max(0) as u64;
        let (year, month, day, hour, minute, dow) = decompose(local);
        // Standard cron day rule (claude-code `cron.ts`): when BOTH day-of-month
        // and day-of-week are restricted, the day matches if EITHER matches; when
        // one is `*` (its expanded set covers the whole domain — len 31 / 7), only
        // the other constrains.
        let dom_wild = self.dom.0.len() == 31;
        let dow_wild = self.dow.0.len() == 7;
        let day_matches = match (dom_wild, dow_wild) {
            (true, true) => true,
            (false, true) => field_match(&self.dom, day),
            (true, false) => field_match(&self.dow, dow),
            (false, false) => field_match(&self.dom, day) || field_match(&self.dow, dow),
        };
        field_match(&self.minute, minute)
            && field_match(&self.hour, hour)
            && field_match(&self.month, month)
            && day_matches
            && year > 1970
    }
}

/// Tool name byte-lock.
pub const CRON_CREATE_TOOL_NAME: &str = "CronCreate";
/// Legacy per-job subdirectory under `~/.claude/` (retained as a wire-identifier
/// lock; the durable path is now the single project file
/// [`cron::tasks_file::SCHEDULED_TASKS_FILE`]).
pub const CRON_SUBDIR: &str = "cron";
/// Legacy per-job file extension (retained as a wire-identifier lock).
pub const CRON_FILE_SUFFIX: &str = ".json";
/// 9-char task-id prefix character (`d` for daemon/cron).
pub const CRON_TASK_ID_PREFIX: char = 'd';
/// 6-field rejection message (retained for the wire-identifier parity lock).
pub const SIX_FIELD_REJECTION: &str =
    "ScheduleCron: 6-field cron (with seconds) is not supported; use 5-field cron (minute hour day month weekday)";
/// Search horizon for `next_fire_unix_secs`: 1 year of minutes (sane upper
/// bound so an unsatisfiable expression doesn't loop forever).
pub const NEXT_FIRE_HORIZON_MINUTES: u64 = 60 * 24 * 366;
/// Maximum number of scheduled jobs allowed at once (CronCreateTool.ts:25).
const MAX_JOBS: usize = 50;
/// Recurring jobs auto-expire after this many days (CronCreateTool.ts prompt.ts).
const DEFAULT_MAX_AGE_DAYS: i64 = 30;

/// Absolute path to the single project tasks file
/// (`<project_root>/.claude/scheduled_tasks.json`). 1:1 with claude-code, which
/// keys cron persistence off the project root (the session cwd), NOT the user
/// config-home. Thin re-export of [`cron::tasks_file::scheduled_tasks_path`] so
/// every cron tool resolves the path identically.
#[must_use]
pub(crate) fn cron_file_path(project_root: &Path) -> PathBuf {
    cron::tasks_file::scheduled_tasks_path(project_root)
}

#[must_use]
pub(crate) fn generate_cron_task_id() -> String {
    use rand::Rng;
    const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut rng = rand::rng();
    let mut s = String::with_capacity(9);
    s.push(CRON_TASK_ID_PREFIX);
    for _ in 0..8 {
        let idx = rng.random_range(0..ALPHABET.len());
        s.push(ALPHABET[idx] as char);
    }
    s
}

/// Compute the next fire time at or after `from` matching `expr`, capped by
/// [`NEXT_FIRE_HORIZON_MINUTES`]. Returns `None` if no minute in the horizon
/// satisfies the expression (the "no calendar date in the next year" case).
fn next_fire_after(expr: &CronExpression, from: SystemTime) -> Option<SystemTime> {
    // Round up to the next minute boundary.
    let from_secs = from.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_secs();
    let mut candidate_secs = (from_secs / 60 + 1) * 60;
    for _ in 0..NEXT_FIRE_HORIZON_MINUTES {
        let candidate = SystemTime::UNIX_EPOCH + Duration::from_secs(candidate_secs);
        if expr.matches(candidate) {
            return Some(candidate);
        }
        candidate_secs += 60;
    }
    None
}

/// Reject 6-field expressions (Quartz-style with seconds). Retained from the
/// previous `ScheduleCron` seam for the wire-identifier parity lock; the live
/// `validate_input`/`call` paths now rely on the shared 5-field parser, which
/// surfaces 6-field input as the generic "Expected 5 fields" message (matching
/// `parseCronExpression` returning null in CronCreateTool.ts).
#[allow(dead_code)]
fn reject_six_field(expr: &str) -> Result<(), ToolError> {
    let parts: usize = expr.split_whitespace().count();
    if parts == 6 {
        return Err(ToolError::InvalidInput(SIX_FIELD_REJECTION.into()));
    }
    Ok(())
}

// -- cronToHuman ------------------------------------------------------------
// Port of `cronToHuman` from claude-code utils/cron.ts. Narrow by design:
// covers the common patterns and falls through to the raw cron string for
// anything else (matches the TS `return cron`).

const DAY_NAMES: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// `true` if `s` is one or more ASCII digits (mirrors the TS `/^\d+$/`).
fn all_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// `true` if `s` is exactly one ASCII digit (mirrors the TS `/^\d$/`).
fn is_single_digit(s: &str) -> bool {
    s.len() == 1 && s.as_bytes()[0].is_ascii_digit()
}

/// Extract `N` from a `*/N` step field (mirrors the TS `/^\*\/(\d+)$/`).
fn parse_step(s: &str) -> Option<u32> {
    s.strip_prefix("*/").and_then(|rest| rest.parse::<u32>().ok())
}

/// Format `hour:minute` (24h) as a 12-hour clock like "2:30pm".
// PARITY-GAP: TS formats in local time (`toLocaleTimeString`); we use UTC to
// avoid local-timezone nondeterminism, and emit lowercase am/pm with no space.
fn format_time_utc(minute: u32, hour: u32) -> String {
    let period = if hour < 12 { "am" } else { "pm" };
    let h12 = match hour % 12 {
        0 => 12,
        h => h,
    };
    format!("{h12}:{minute:02}{period}")
}

/// Render a 5-field cron expression as a human-readable schedule string.
pub(crate) fn cron_to_human(cron: &str) -> String {
    let parts: Vec<&str> = cron.split_whitespace().collect();
    if parts.len() != 5 {
        return cron.to_string();
    }
    let (minute, hour, dom, month, dow) = (parts[0], parts[1], parts[2], parts[3], parts[4]);

    // Every N minutes: */N * * * *
    if let Some(n) = parse_step(minute) {
        if hour == "*" && dom == "*" && month == "*" && dow == "*" {
            return if n == 1 {
                "Every minute".to_string()
            } else {
                format!("Every {n} minutes")
            };
        }
    }

    // Every hour: M * * * *
    if all_digits(minute) && hour == "*" && dom == "*" && month == "*" && dow == "*" {
        let m: u32 = minute.parse().unwrap_or(0);
        if m == 0 {
            return "Every hour".to_string();
        }
        return format!("Every hour at :{m:02}");
    }

    // Every N hours: M */N * * *
    if all_digits(minute) {
        if let Some(n) = parse_step(hour) {
            if dom == "*" && month == "*" && dow == "*" {
                let m: u32 = minute.parse().unwrap_or(0);
                let suffix = if m == 0 {
                    String::new()
                } else {
                    format!(" at :{m:02}")
                };
                return if n == 1 {
                    format!("Every hour{suffix}")
                } else {
                    format!("Every {n} hours{suffix}")
                };
            }
        }
    }

    // Remaining cases reference hour+minute and require both to be numeric.
    if !all_digits(minute) || !all_digits(hour) {
        return cron.to_string();
    }
    let m: u32 = minute.parse().unwrap_or(0);
    let h: u32 = hour.parse().unwrap_or(0);
    let time = format_time_utc(m, h);

    // Daily at specific time: M H * * *
    if dom == "*" && month == "*" && dow == "*" {
        return format!("Every day at {time}");
    }

    // Specific day of week: M H * * D
    if dom == "*" && month == "*" && is_single_digit(dow) {
        let day_index = (dow.parse::<usize>().unwrap_or(0)) % 7; // normalize 7 -> 0
        if let Some(name) = DAY_NAMES.get(day_index) {
            return format!("Every {name} at {time}");
        }
    }

    // Weekdays: M H * * 1-5
    if dom == "*" && month == "*" && dow == "1-5" {
        return format!("Weekdays at {time}");
    }

    // PARITY-GAP: extensions beyond TS cronToHuman (which returns the raw cron
    // for these). Specific month + day-of-month: M H D Mon *
    if all_digits(dom) && all_digits(month) && dow == "*" {
        let mon: usize = month.parse().unwrap_or(0);
        if (1..=12).contains(&mon) {
            let name = MONTH_NAMES[mon - 1];
            return format!("{name} {dom} at {time}");
        }
    }

    // PARITY-GAP: extension beyond TS. Day-of-month, every month: M H D * *
    if all_digits(dom) && month == "*" && dow == "*" {
        return format!("Day {dom} of every month at {time}");
    }

    cron.to_string()
}

/// Parse a semantic boolean: accepts a JSON bool, or the strings
/// "true"/"false"/"yes"/"no"/"1"/"0" (case-insensitive). Anything else
/// (missing, null, unrecognized string, non-bool) falls back to `default`.
fn semantic_bool(v: Option<&Value>, default: bool) -> bool {
    match v {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "yes" | "1" => true,
            "false" | "no" | "0" => false,
            _ => default,
        },
        _ => default,
    }
}

/// Build the model-facing result text (CronCreateTool.ts:143-153).
fn build_result_content(id: &str, human: &str, recurring: bool, durable: bool) -> String {
    let where_ = if durable {
        "Persisted to .claude/scheduled_tasks.json"
    } else {
        "Session-only (not written to disk, dies when Claude exits)"
    };
    if recurring {
        format!(
            "Scheduled recurring job {id} ({human}). {where_}. Auto-expires after {DEFAULT_MAX_AGE_DAYS} days. Use CronDelete to cancel sooner."
        )
    } else {
        format!("Scheduled one-shot task {id} ({human}). {where_}. It will fire once then auto-delete.")
    }
}

/// Count persisted jobs in the project's single `scheduled_tasks.json`
/// (`tasks.len()`). A missing/garbage file counts as zero. Backs the
/// `MAX_JOBS = 50` limit (CronCreateTool.ts:25).
fn count_existing_jobs(project_root: &Path) -> usize {
    let path = cron_file_path(project_root);
    match std::fs::read_to_string(&path) {
        Ok(body) => cron::tasks_file::parse_tasks(&body).tasks.len(),
        Err(_) => 0,
    }
}

/// `CronCreateTool` — schedule a prompt on a 5-field cron + persist the job.
pub struct CronCreateTool {
    pub(crate) ctx: tool_api::BuiltinToolContext,
}

impl CronCreateTool {
    /// Construct.
    #[must_use]
    pub fn new(ctx: tool_api::BuiltinToolContext) -> Self {
        Self { ctx }
    }
}

static SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "cron": {
                "type": "string",
                "description": "Standard 5-field cron expression in local time: \"M H DoM Mon DoW\" (e.g. \"*/5 * * * *\" = every 5 minutes, \"30 14 28 2 *\" = Feb 28 at 2:30pm local once)."
            },
            "prompt": {
                "type": "string",
                "description": "The prompt to enqueue at each fire time."
            },
            "recurring": {
                "type": "boolean",
                "description": format!("true (default) = fire on every cron match until deleted or auto-expired after {DEFAULT_MAX_AGE_DAYS} days. false = fire once at the next match, then auto-delete. Use false for \"remind me at X\" one-shot requests with pinned minute/hour/dom/month.")
            },
            "durable": {
                "type": "boolean",
                "description": "true = persist to .claude/scheduled_tasks.json and survive restarts. false (default) = in-memory only, dies when this Claude session ends. Use true only when the user asks the task to survive across sessions."
            }
        },
        "required": ["cron", "prompt"]
    })
});

fn pii_str(s: &str) -> AnalyticsValue {
    AnalyticsValue::String(PiiTagged::assert_pii_tagged_column(s.to_string()).into_inner())
}

fn verified_str(s: &str) -> AnalyticsValue {
    AnalyticsValue::String(Verified::assert_safe(s.to_string()).into_inner())
}

async fn emit_failed(bus: &Arc<AnalyticsBus>, kind: &str, duration_ms: u64) {
    let mut md: LogEventMetadata = HashMap::new();
    md.insert("error_kind".into(), verified_str(kind));
    md.insert(
        "duration_ms".into(),
        AnalyticsValue::Int(duration_ms as i64),
    );
    bus.log_event(SCHEDULE_CRON_FAILED, md).await;
}

#[async_trait]
impl Tool for CronCreateTool {
    fn name(&self) -> &str {
        CRON_CREATE_TOOL_NAME
    }
    fn input_schema(&self) -> &Value {
        &SCHEMA
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        100_000
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        true
    }
    fn is_read_only(&self, _: &Value) -> bool {
        false
    }
    fn is_destructive(&self, _: &Value) -> bool {
        false
    }
    fn is_open_world(&self, _: &Value) -> bool {
        false
    }
    fn interrupt_behavior(&self, _: &Value) -> InterruptBehavior {
        InterruptBehavior::Block
    }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "CronCreate persists a cron job to .claude/scheduled_tasks.json".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "Schedule a prompt to run on a cron schedule.".into()
    }

    async fn prompt(&self, _: &PromptOptions) -> String {
        "CronCreate: schedule a recurring or one-shot prompt via a 5-field cron expression.".into()
    }

    async fn validate_input(
        &self,
        input: &Value,
        _: &ToolUseContext,
    ) -> Result<(), ValidationError> {
        let cron = input
            .get("cron")
            .and_then(Value::as_str)
            .ok_or_else(|| ValidationError("CronCreate: missing or non-string cron".into()))?;

        // Invalid cron (not 5 fields / parse fail).
        let parsed = match parse_cron(cron) {
            Ok(p) => p,
            Err(_) => {
                return Err(ValidationError(format!(
                    "Invalid cron expression '{cron}'. Expected 5 fields: M H DoM Mon DoW."
                )));
            }
        };

        // No calendar date in the next year satisfies the expression.
        let now = self.ctx.clock.now();
        if next_fire_after(&parsed, now).is_none() {
            return Err(ValidationError(format!(
                "Cron expression '{cron}' does not match any calendar date in the next year."
            )));
        }

        // Too many scheduled jobs already (counted in the single project file).
        if count_existing_jobs(&self.ctx.workspace) >= MAX_JOBS {
            return Err(ValidationError(format!(
                "Too many scheduled jobs (max {MAX_JOBS}). Cancel one first."
            )));
        }

        // PARITY-GAP: TS rejects a `durable` cron created by a teammate
        // (`input.durable && getTeammateContext()`) because teammates don't
        // persist across sessions. There is no teammate context in this Rust
        // seam, so the durable+teammate check is omitted.
        Ok(())
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let started = Instant::now();
        let bus = self.ctx.bus.clone();

        let cron = match input.get("cron").and_then(Value::as_str) {
            Some(s) => s.to_string(),
            None => {
                emit_failed(&bus, "missing_cron", started.elapsed().as_millis() as u64).await;
                return Err(ToolError::InvalidInput(
                    "CronCreate: missing or non-string cron".into(),
                ));
            }
        };
        let prompt = match input.get("prompt").and_then(Value::as_str) {
            Some(s) => s.to_string(),
            None => {
                emit_failed(&bus, "missing_prompt", started.elapsed().as_millis() as u64).await;
                return Err(ToolError::InvalidInput(
                    "CronCreate: missing or non-string prompt".into(),
                ));
            }
        };

        let parsed = match parse_cron(&cron) {
            Ok(c) => c,
            Err(_) => {
                emit_failed(&bus, "parse_error", started.elapsed().as_millis() as u64).await;
                return Err(ToolError::InvalidInput(format!(
                    "Invalid cron expression '{cron}'. Expected 5 fields: M H DoM Mon DoW."
                )));
            }
        };

        let recurring = semantic_bool(input.get("recurring"), true);
        let durable = semantic_bool(input.get("durable"), false);

        let mut md: LogEventMetadata = HashMap::new();
        md.insert("_PROTO_cron".into(), pii_str(&cron));
        md.insert("_PROTO_prompt".into(), pii_str(&prompt));
        bus.log_event(SCHEDULE_CRON_STARTED, md).await;

        let now = self.ctx.clock.now();
        // Next-fire is COMPUTED at runtime (cron + createdAt/lastFiredAt) and is
        // never persisted (claude-code parity); we still derive it here purely
        // for the analytics `next_fire_unix_secs` signal.
        let next = next_fire_after(&parsed, now);
        let next_unix = next
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());
        // `createdAt` is epoch MILLISECONDS on disk (claude-code `Date.now()`).
        let created_ms = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let id = generate_cron_task_id();

        // Persist ONLY when durable. A session-only job (durable:false) is never
        // written to disk — it would live in claude-code's separate in-memory
        // session store (which this seam does not own), honoring the tool's
        // "Session-only (not written to disk …)" promise.
        let mut bytes_written: usize = 0;
        if durable {
            let path = cron_file_path(&self.ctx.workspace);
            if let Some(dir) = path.parent() {
                if let Err(e) = tokio::fs::create_dir_all(dir).await {
                    emit_failed(&bus, "io_create_dir", started.elapsed().as_millis() as u64).await;
                    return Err(ToolError::Io(format!(
                        "CronCreate: io error at {}: {e}",
                        dir.display()
                    )));
                }
            }
            // Read-modify-write the single `{ "tasks": [...] }` document: load the
            // existing tasks (empty if absent/garbage), append the new CronTask,
            // and write the whole file back.
            let mut doc = match tokio::fs::read_to_string(&path).await {
                Ok(body) => cron::tasks_file::parse_tasks(&body),
                Err(_) => cron::tasks_file::ScheduledTasks::default(),
            };
            doc.tasks.push(cron::tasks_file::CronTask {
                id: id.clone(),
                cron: cron.clone(),
                prompt: prompt.clone(),
                created_at: created_ms,
                last_fired_at: None,
                recurring: Some(recurring),
                permanent: None,
            });
            let body = cron::tasks_file::serialize_tasks(&doc);
            bytes_written = body.len();
            if let Err(e) = tokio::fs::write(&path, body.as_bytes()).await {
                emit_failed(&bus, "io_write", started.elapsed().as_millis() as u64).await;
                return Err(ToolError::Io(format!(
                    "CronCreate: io error at {}: {e}",
                    path.display()
                )));
            }
        }

        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(started.elapsed().as_millis() as i64),
        );
        md.insert(
            "next_fire_unix_secs".into(),
            match next_unix {
                Some(n) => AnalyticsValue::Int(n as i64),
                None => AnalyticsValue::None,
            },
        );
        md.insert(
            "bytes_written".into(),
            AnalyticsValue::Int(bytes_written as i64),
        );
        bus.log_event(SCHEDULE_CRON_COMPLETED, md).await;

        let human = cron_to_human(&cron);
        let content = build_result_content(&id, &human, recurring, durable);

        Ok(ToolCallResult {
            data: json!({
                "id": id,
                "humanSchedule": human,
                "recurring": recurring,
                "durable": durable,
                "content": content,
            }),
            new_messages: vec![],
            context_modifier: None,
            mcp_meta: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tool_api::test_support::{fresh_ctx, fresh_tx, shell_test_ctx_in};
    use traits::process::ProcessOutput;

    fn dummy_out() -> ProcessOutput {
        ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        }
    }

    /// Read the parsed `{ "tasks": [...] }` document at
    /// `<root>/.claude/scheduled_tasks.json` (empty if absent).
    async fn read_doc(root: &std::path::Path) -> cron::tasks_file::ScheduledTasks {
        let path = cron::tasks_file::scheduled_tasks_path(root);
        match tokio::fs::read_to_string(&path).await {
            Ok(body) => cron::tasks_file::parse_tasks(&body),
            Err(_) => cron::tasks_file::ScheduledTasks::default(),
        }
    }

    #[test]
    fn constants_locked() {
        assert_eq!(CRON_CREATE_TOOL_NAME, "CronCreate");
        assert_eq!(CRON_SUBDIR, "cron");
        assert_eq!(CRON_TASK_ID_PREFIX, 'd');
        assert_eq!(MAX_JOBS, 50);
        assert_eq!(DEFAULT_MAX_AGE_DAYS, 30);
    }

    #[test]
    fn task_id_format() {
        let id = generate_cron_task_id();
        assert_eq!(id.len(), 9);
        assert_eq!(id.chars().next().unwrap(), 'd');
    }

    #[test]
    fn rejects_six_field_with_seconds() {
        let r = reject_six_field("0 */5 9-17 * * 1-5");
        let err = r.unwrap_err();
        assert!(format!("{err}").contains("6-field cron"));
    }

    #[test]
    fn semantic_bool_parsing() {
        // Missing -> default.
        assert!(semantic_bool(None, true));
        assert!(!semantic_bool(None, false));
        // JSON bool.
        assert!(semantic_bool(Some(&json!(true)), false));
        assert!(!semantic_bool(Some(&json!(false)), true));
        // Truthy strings (case-insensitive).
        assert!(semantic_bool(Some(&json!("yes")), false));
        assert!(semantic_bool(Some(&json!("TRUE")), false));
        assert!(semantic_bool(Some(&json!("1")), false));
        // Falsy strings (case-insensitive).
        assert!(!semantic_bool(Some(&json!("no")), true));
        assert!(!semantic_bool(Some(&json!("FALSE")), true));
        assert!(!semantic_bool(Some(&json!("0")), true));
        // Unrecognized / non-bool -> default.
        assert!(semantic_bool(Some(&json!("maybe")), true));
        assert!(!semantic_bool(Some(&json!("maybe")), false));
        assert!(!semantic_bool(Some(&json!(5)), false));
    }

    #[test]
    fn cron_to_human_cases() {
        assert_eq!(cron_to_human("*/5 * * * *"), "Every 5 minutes");
        assert_eq!(cron_to_human("*/1 * * * *"), "Every minute");
        assert_eq!(cron_to_human("0 * * * *"), "Every hour");
        assert_eq!(cron_to_human("30 * * * *"), "Every hour at :30");
        assert_eq!(cron_to_human("0 */2 * * *"), "Every 2 hours");
        assert_eq!(cron_to_human("0 9 * * *"), "Every day at 9:00am");
        assert_eq!(cron_to_human("0 12 * * *"), "Every day at 12:00pm");
        assert_eq!(cron_to_human("30 14 * * 1"), "Every Monday at 2:30pm");
        assert_eq!(cron_to_human("0 0 * * 0"), "Every Sunday at 12:00am");
        assert_eq!(cron_to_human("0 9 * * 1-5"), "Weekdays at 9:00am");
        assert_eq!(cron_to_human("30 14 28 2 *"), "February 28 at 2:30pm");
        assert_eq!(cron_to_human("0 9 15 * *"), "Day 15 of every month at 9:00am");
        // Unrecognized -> raw cron string.
        assert_eq!(cron_to_human("garbage"), "garbage");
        assert_eq!(cron_to_human("* * * * *"), "* * * * *");
    }

    #[test]
    fn result_content_variants() {
        let r = build_result_content("d12345678", "Every day at 9:00am", true, false);
        assert!(r.contains("Scheduled recurring job d12345678 (Every day at 9:00am)."));
        assert!(r.contains("Session-only (not written to disk, dies when Claude exits)"));
        assert!(r.contains("Auto-expires after 30 days. Use CronDelete to cancel sooner."));

        let o = build_result_content("d87654321", "February 28 at 2:30pm", false, true);
        assert!(o.contains("Scheduled one-shot task d87654321 (February 28 at 2:30pm)."));
        assert!(o.contains("Persisted to .claude/scheduled_tasks.json"));
        assert!(o.contains("It will fire once then auto-delete."));
    }

    #[tokio::test]
    async fn session_only_job_is_not_written_to_disk() {
        // A durable:false (default) job lives only in the (separate) in-memory
        // session store; nothing is written to scheduled_tasks.json.
        let tmp = tempfile::tempdir().unwrap();
        let tool = CronCreateTool::new(shell_test_ctx_in(dummy_out(), tmp.path().to_path_buf()));
        let out = tool
            .call(
                json!({"cron": "*/5 9-17 * * 1-5", "prompt": "echo hi"}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ok");
        let id = out.data["id"].as_str().unwrap();
        assert!(id.starts_with('d') && id.len() == 9, "{id}");
        // Defaults: recurring=true, durable=false.
        assert_eq!(out.data["recurring"], json!(true));
        assert_eq!(out.data["durable"], json!(false));
        let content = out.data["content"].as_str().unwrap();
        assert!(content.contains("Scheduled recurring job"));
        assert!(content.contains("Session-only (not written to disk, dies when Claude exits)"));

        // No file written at all.
        let path = cron::tasks_file::scheduled_tasks_path(tmp.path());
        assert!(!tokio::fs::try_exists(&path).await.unwrap());
        assert!(read_doc(tmp.path()).await.tasks.is_empty());
    }

    #[tokio::test]
    async fn durable_job_persists_to_single_project_file() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = CronCreateTool::new(shell_test_ctx_in(dummy_out(), tmp.path().to_path_buf()));
        let out = tool
            .call(
                json!({"cron": "*/5 9-17 * * 1-5", "prompt": "echo hi", "durable": true}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ok");
        let id = out.data["id"].as_str().unwrap().to_string();

        // The single `{ "tasks": [...] }` file carries one CronTask with the
        // camelCase, epoch-MILLISECONDS shape — and NO durable / next-fire keys.
        let path = cron::tasks_file::scheduled_tasks_path(tmp.path());
        let written = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(written.contains("\"tasks\""));
        assert!(written.contains("\"cron\": \"*/5 9-17 * * 1-5\""));
        assert!(written.contains("\"prompt\": \"echo hi\""));
        assert!(written.contains("\"createdAt\""));
        assert!(written.contains("\"recurring\": true"));
        assert!(!written.contains("durable"));
        assert!(!written.contains("nextFire"));
        assert!(!written.contains("next_fire"));
        assert!(!written.contains("created_at_unix"));
        // Trailing newline (claude-code `+ '\n'`).
        assert!(written.ends_with("}\n"));

        let doc = read_doc(tmp.path()).await;
        assert_eq!(doc.tasks.len(), 1);
        assert_eq!(doc.tasks[0].id, id);
        assert_eq!(doc.tasks[0].recurring, Some(true));
        assert_eq!(doc.tasks[0].last_fired_at, None);
        // `createdAt` is derived from the clock in epoch MILLISECONDS. The test
        // StubClock is anchored at the Unix epoch, so this is 0 here — the ms
        // conversion (`as_millis`) is exercised by the scheduler round-trip tests
        // with a non-epoch clock.
        assert_eq!(doc.tasks[0].created_at, 0);
        assert!(written.contains("\"createdAt\": 0"));
    }

    #[tokio::test]
    async fn durable_creates_append_into_one_file() {
        // Two durable creates accumulate in the SAME file (read-modify-write).
        let tmp = tempfile::tempdir().unwrap();
        let tool = CronCreateTool::new(shell_test_ctx_in(dummy_out(), tmp.path().to_path_buf()));
        for cron in ["*/5 * * * *", "0 9 * * *"] {
            tool.call(
                json!({"cron": cron, "prompt": "p", "durable": true}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ok");
        }
        let doc = read_doc(tmp.path()).await;
        assert_eq!(doc.tasks.len(), 2);
    }

    #[tokio::test]
    async fn one_shot_durable_result() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = CronCreateTool::new(shell_test_ctx_in(dummy_out(), tmp.path().to_path_buf()));
        let out = tool
            .call(
                json!({"cron": "30 14 28 2 *", "prompt": "remind me", "recurring": false, "durable": true}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ok");
        assert_eq!(out.data["recurring"], json!(false));
        assert_eq!(out.data["durable"], json!(true));
        assert_eq!(out.data["humanSchedule"], json!("February 28 at 2:30pm"));
        let content = out.data["content"].as_str().unwrap();
        assert!(content.contains("Scheduled one-shot task"));
        assert!(content.contains("It will fire once then auto-delete"));
        assert!(content.contains("Persisted to .claude/scheduled_tasks.json"));
        // recurring:false → the optional `recurring` key is omitted on disk.
        let doc = read_doc(tmp.path()).await;
        assert_eq!(doc.tasks.len(), 1);
        assert_eq!(doc.tasks[0].recurring, Some(false));
    }

    #[tokio::test]
    async fn semantic_string_flags_via_call() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = CronCreateTool::new(shell_test_ctx_in(dummy_out(), tmp.path().to_path_buf()));
        let out = tool
            .call(
                json!({"cron": "0 9 * * *", "prompt": "x", "recurring": "no", "durable": "yes"}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ok");
        assert_eq!(out.data["recurring"], json!(false));
        assert_eq!(out.data["durable"], json!(true));
    }

    #[tokio::test]
    async fn call_rejects_garbage_cron() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = CronCreateTool::new(shell_test_ctx_in(dummy_out(), tmp.path().to_path_buf()));
        let err = tool
            .call(
                json!({"cron": "not a cron", "prompt": "x"}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect_err("garbage");
        assert!(format!("{err}").contains("Invalid cron expression 'not a cron'. Expected 5 fields"));
    }

    #[tokio::test]
    async fn call_rejects_six_field_as_invalid() {
        // PARITY: 6-field is no longer a special message; the shared 5-field
        // parser surfaces it as the generic "Expected 5 fields" error (TS
        // `parseCronExpression` returns null for non-5-field input).
        let tmp = tempfile::tempdir().unwrap();
        let tool = CronCreateTool::new(shell_test_ctx_in(dummy_out(), tmp.path().to_path_buf()));
        let err = tool
            .call(
                json!({"cron": "0 */5 9-17 * * 1-5", "prompt": "x"}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect_err("must reject");
        assert!(format!("{err}").contains("Invalid cron expression"));
        assert!(format!("{err}").contains("Expected 5 fields"));
    }

    #[tokio::test]
    async fn validate_rejects_garbage_cron() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = CronCreateTool::new(shell_test_ctx_in(dummy_out(), tmp.path().to_path_buf()));
        let err = tool
            .validate_input(&json!({"cron": "not a cron", "prompt": "x"}), &fresh_ctx())
            .await
            .expect_err("garbage");
        assert!(err
            .0
            .contains("Invalid cron expression 'not a cron'. Expected 5 fields: M H DoM Mon DoW."));
    }

    // -- CRON.3: day-of-week 7 (Sunday alias) normalizes to 0 -----------------
    #[test]
    fn dow_seven_normalizes_to_sunday() {
        // Bare 7 -> 0 (both mean Sunday).
        let e = parse_cron("0 0 * * 7").expect("dow 7 accepted");
        assert_eq!(e.dow.0, vec![0]);
        // 7 at the top of a range expands with 7 folded to 0 (5-7 = Fri,Sat,Sun).
        let e = parse_cron("0 0 * * 5-7").expect("range ending at 7");
        assert_eq!(e.dow.0, vec![0, 5, 6]);
        // 0 and 7 collapse to a single Sunday entry.
        let e = parse_cron("0 0 * * 0,7").expect("0 and 7 dedupe");
        assert_eq!(e.dow.0, vec![0]);
        // 7 is a Sunday alias ONLY for day-of-week; elsewhere it is a plain
        // value and is NOT rewritten to 0.
        assert_eq!(parse_cron("7 0 * * *").unwrap().minute.0, vec![7]);
        assert_eq!(parse_cron("0 0 * 7 *").unwrap().month.0, vec![7]);
    }

    // -- CRON.4: mixed list / range / step forms in one field -----------------
    #[test]
    fn mixed_list_range_step_forms() {
        // List + range + single composed in the day-of-week field.
        let e = parse_cron("0 9 * * 1-5,0").expect("mixed dow");
        assert_eq!(e.dow.0, vec![0, 1, 2, 3, 4, 5]);
        // Stepped range plus an explicit single value.
        let e = parse_cron("0-30/10,45 * * * *").expect("stepped range + single");
        assert_eq!(e.minute.0, vec![0, 10, 20, 30, 45]);
        // Wildcard step combined with a single value (deduped + sorted).
        let e = parse_cron("*/15,7 * * * *").expect("wildcard step + single");
        assert_eq!(e.minute.0, vec![0, 7, 15, 30, 45]);
        // The previous parser rejected "1-5,0" (range branch swallowed the
        // comma); confirm the whole expression now parses end-to-end.
        assert!(parse_cron("30 14 * * 1-5,0").is_ok());
    }

    // -- CRON.2: per-field range validation -----------------------------------
    #[test]
    fn out_of_range_values_rejected() {
        // One past the top of each field's range.
        assert!(parse_cron("60 * * * *").is_err()); // minute max 59
        assert!(parse_cron("* 24 * * *").is_err()); // hour max 23
        assert!(parse_cron("* * 0 * *").is_err()); // dom min 1
        assert!(parse_cron("* * 32 * *").is_err()); // dom max 31
        assert!(parse_cron("* * * 0 *").is_err()); // month min 1
        assert!(parse_cron("* * * 13 *").is_err()); // month max 12
        assert!(parse_cron("* * * * 8").is_err()); // dow max 7 (alias), 8 invalid
        // Out-of-range hidden inside a list / range is rejected too.
        assert!(parse_cron("0,60 * * * *").is_err());
        assert!(parse_cron("* * * * 5-8").is_err());
        // Boundary values (including the dow 7 alias) are accepted.
        assert!(parse_cron("59 23 31 12 7").is_ok());
    }

    #[tokio::test]
    async fn validate_rejects_out_of_range_with_invalid_message() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = CronCreateTool::new(shell_test_ctx_in(dummy_out(), tmp.path().to_path_buf()));
        // Minute 99 is out of range -> the parser fails -> the generic
        // byte-exact "Invalid cron expression" message (errorCode 1 in TS),
        // NOT the "does not match any calendar date" message (errorCode 2).
        let err = tool
            .validate_input(&json!({"cron": "99 * * * *", "prompt": "x"}), &fresh_ctx())
            .await
            .expect_err("out of range");
        assert_eq!(
            err.0,
            "Invalid cron expression '99 * * * *'. Expected 5 fields: M H DoM Mon DoW."
        );
    }

    #[tokio::test]
    async fn validate_rejects_unsatisfiable_cron() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = CronCreateTool::new(shell_test_ctx_in(dummy_out(), tmp.path().to_path_buf()));
        // February 30 is a real impossible date (Feb has at most 29 days), so no
        // calendar date in the horizon matches. (DOM 31 — which the old crude
        // 30-day decompose wrongly rejected — is now correctly satisfiable; see
        // `day_of_month_31_is_satisfiable`.)
        let err = tool
            .validate_input(&json!({"cron": "0 0 30 2 *", "prompt": "x"}), &fresh_ctx())
            .await
            .expect_err("unsatisfiable");
        assert!(err
            .0
            .contains("does not match any calendar date in the next year"));
        // NOTE: DOM 31 (which the old crude 30-day decompose wrongly rejected) is
        // now correctly satisfiable on the real calendar — proven directly against
        // the matcher in `cron::schedule::tests::day_of_month_31_can_match`. It
        // can't be asserted *here* because the test `StubClock` is anchored at the
        // Unix epoch and the matcher's `year > 1970` sentinel keeps the 366-day
        // horizon (all in 1970) from matching any narrow date.
    }
}
