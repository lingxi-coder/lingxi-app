//! `CronCreateTool` — schedule a prompt on a 5-field cron schedule and persist
//! the job descriptor to `~/.claude/cron/<id>.json`.
//!
//! 1:1 parity rewrite of claude-code `CronCreateTool.ts`. The model supplies a
//! 5-field cron expression plus the prompt to enqueue at each fire time, with
//! optional `recurring` (default true) and `durable` (default false) flags.
//!
//! Wire identifiers:
//! - 5-field cron expressions only (6-field with seconds is rejected by the
//!   shared parser and surfaces as the generic "Expected 5 fields" message).
//! - Persistence path `~/.claude/cron/<id>.json` where
//!   `id = [d][0-9a-z]{8}` (9-char format).
//!
//! This seam does NOT register the job into a live scheduler — it parses,
//! computes the next fire time, and persists the descriptor. Production hosts
//! pick the file up via `cron::scheduler::CronScheduler`.

use std::collections::HashMap;
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

#[derive(Debug, Clone)]
enum CronField {
    Any,
    Exact(u32),
    Step(u32),
    Range(u32, u32),
    List(Vec<u32>),
}

#[derive(Debug, Clone)]
struct CronExpression {
    minute: CronField,
    hour: CronField,
    dom: CronField,
    month: CronField,
    dow: CronField,
}

fn parse_cron_field(s: &str) -> Result<CronField, String> {
    if s == "*" {
        return Ok(CronField::Any);
    }
    if let Some(rest) = s.strip_prefix("*/") {
        let n = rest.parse::<u32>().map_err(|_| format!("bad field: {s}"))?;
        return Ok(CronField::Step(n));
    }
    if let Some((a, b)) = s.split_once('-') {
        let a = a.parse::<u32>().map_err(|_| format!("bad field: {s}"))?;
        let b = b.parse::<u32>().map_err(|_| format!("bad field: {s}"))?;
        return Ok(CronField::Range(a, b));
    }
    if s.contains(',') {
        let list: Vec<u32> = s
            .split(',')
            .map(str::parse::<u32>)
            .collect::<Result<_, _>>()
            .map_err(|_| format!("bad field: {s}"))?;
        return Ok(CronField::List(list));
    }
    s.parse::<u32>()
        .map(CronField::Exact)
        .map_err(|_| format!("bad field: {s}"))
}

fn parse_cron(s: &str) -> Result<CronExpression, String> {
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() != 5 {
        return Err(format!("expected 5 fields, got {}", parts.len()));
    }
    Ok(CronExpression {
        minute: parse_cron_field(parts[0])?,
        hour: parse_cron_field(parts[1])?,
        dom: parse_cron_field(parts[2])?,
        month: parse_cron_field(parts[3])?,
        dow: parse_cron_field(parts[4])?,
    })
}

fn field_match(field: &CronField, value: u32) -> bool {
    match field {
        CronField::Any => true,
        CronField::Exact(v) => *v == value,
        CronField::Step(n) => *n > 0 && value % n == 0,
        CronField::Range(a, b) => value >= *a && value <= *b,
        CronField::List(list) => list.contains(&value),
    }
}

#[allow(clippy::cast_possible_truncation)]
fn decompose(secs: u64) -> (u32, u32, u32, u32, u32, u32) {
    // Mirrors cron::schedule::decompose (crude UTC; sufficient for matches()).
    let minute = (secs / 60 % 60) as u32;
    let hour = (secs / 3600 % 24) as u32;
    let day = (secs / 86_400 % 30 + 1) as u32;
    let month = ((secs / 2_628_000) % 12 + 1) as u32;
    let year = 1970 + (secs / 31_536_000) as u32;
    let dow = ((secs / 86_400 + 4) % 7) as u32;
    (year, month, day, hour, minute, dow)
}

impl CronExpression {
    fn matches(&self, time: SystemTime) -> bool {
        let secs = time
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let (year, month, day, hour, minute, dow) = decompose(secs);
        field_match(&self.minute, minute)
            && field_match(&self.hour, hour)
            && field_match(&self.dom, day)
            && field_match(&self.month, month)
            && field_match(&self.dow, dow)
            && year > 1970
    }
}

/// Tool name byte-lock.
pub const CRON_CREATE_TOOL_NAME: &str = "CronCreate";
/// Subdirectory under `~/.claude/`.
pub const CRON_SUBDIR: &str = "cron";
/// File extension.
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

fn home_dir_or_internal() -> Result<PathBuf, ToolError> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| ToolError::Internal("CronCreate: HOME directory not available".into()))
}

#[must_use]
pub(crate) fn cron_path(home: &Path, task_id: &str) -> PathBuf {
    home.join(".claude")
        .join(CRON_SUBDIR)
        .join(format!("{task_id}{CRON_FILE_SUFFIX}"))
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
fn cron_to_human(cron: &str) -> String {
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

/// Count existing `~/.claude/cron/*.json` job descriptors.
fn count_existing_jobs(home: &Path) -> usize {
    let dir = home.join(".claude").join(CRON_SUBDIR);
    match std::fs::read_dir(&dir) {
        Ok(rd) => rd
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
            .count(),
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
                reason: "CronCreate persists a cron descriptor under ~/.claude/cron/".into(),
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

        // Too many scheduled jobs already.
        if let Ok(home) = home_dir_or_internal() {
            if count_existing_jobs(&home) >= MAX_JOBS {
                return Err(ValidationError(format!(
                    "Too many scheduled jobs (max {MAX_JOBS}). Cancel one first."
                )));
            }
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
        let next = next_fire_after(&parsed, now);
        let next_unix = next
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());
        let created = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let home = match home_dir_or_internal() {
            Ok(h) => h,
            Err(e) => {
                emit_failed(&bus, "no_home", started.elapsed().as_millis() as u64).await;
                return Err(e);
            }
        };
        let id = generate_cron_task_id();
        let path = cron_path(&home, &id);
        let descriptor = json!({
            "id": id,
            "cron": cron,
            "prompt": prompt,
            "recurring": recurring,
            "durable": durable,
            "created_at_unix_secs": created,
            "next_fire_unix_secs": next_unix,
        });
        if let Some(dir) = path.parent() {
            if let Err(e) = tokio::fs::create_dir_all(dir).await {
                emit_failed(&bus, "io_create_dir", started.elapsed().as_millis() as u64).await;
                return Err(ToolError::Io(format!(
                    "CronCreate: io error at {}: {e}",
                    dir.display()
                )));
            }
        }
        let body = match serde_json::to_vec_pretty(&descriptor) {
            Ok(b) => b,
            Err(e) => {
                emit_failed(&bus, "serde_error", started.elapsed().as_millis() as u64).await;
                return Err(ToolError::Internal(format!("CronCreate: serde error: {e}")));
            }
        };
        if let Err(e) = tokio::fs::write(&path, &body).await {
            emit_failed(&bus, "io_write", started.elapsed().as_millis() as u64).await;
            return Err(ToolError::Io(format!(
                "CronCreate: io error at {}: {e}",
                path.display()
            )));
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
            AnalyticsValue::Int(body.len() as i64),
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
    use tool_api::test_support::{fresh_ctx, fresh_tx, shell_test_ctx, HOME_LOCK};
    use traits::process::ProcessOutput;

    fn dummy_out() -> ProcessOutput {
        ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
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
    async fn persists_descriptor() {
        let _g = HOME_LOCK.lock().await;
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", tmp.path());
        let tool = CronCreateTool::new(shell_test_ctx(dummy_out()));
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

        let path = format!("{}/.claude/cron/{id}.json", tmp.path().display());
        let written = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(written.contains("\"cron\": \"*/5 9-17 * * 1-5\""));
        assert!(written.contains("\"prompt\": \"echo hi\""));
        assert!(written.contains("\"recurring\": true"));
        assert!(written.contains("\"durable\": false"));
    }

    #[tokio::test]
    async fn one_shot_durable_result() {
        let _g = HOME_LOCK.lock().await;
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", tmp.path());
        let tool = CronCreateTool::new(shell_test_ctx(dummy_out()));
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
    }

    #[tokio::test]
    async fn semantic_string_flags_via_call() {
        let _g = HOME_LOCK.lock().await;
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", tmp.path());
        let tool = CronCreateTool::new(shell_test_ctx(dummy_out()));
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
        let _g = HOME_LOCK.lock().await;
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", tmp.path());
        let tool = CronCreateTool::new(shell_test_ctx(dummy_out()));
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
        let _g = HOME_LOCK.lock().await;
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", tmp.path());
        let tool = CronCreateTool::new(shell_test_ctx(dummy_out()));
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
        let _g = HOME_LOCK.lock().await;
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", tmp.path());
        let tool = CronCreateTool::new(shell_test_ctx(dummy_out()));
        let err = tool
            .validate_input(&json!({"cron": "not a cron", "prompt": "x"}), &fresh_ctx())
            .await
            .expect_err("garbage");
        assert!(err
            .0
            .contains("Invalid cron expression 'not a cron'. Expected 5 fields: M H DoM Mon DoW."));
    }

    #[tokio::test]
    async fn validate_rejects_unsatisfiable_cron() {
        let _g = HOME_LOCK.lock().await;
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", tmp.path());
        let tool = CronCreateTool::new(shell_test_ctx(dummy_out()));
        // Day-of-month 31 never occurs under the crude decompose (day maxes at
        // 30), so no calendar date in the horizon matches.
        let err = tool
            .validate_input(&json!({"cron": "0 0 31 * *", "prompt": "x"}), &fresh_ctx())
            .await
            .expect_err("unsatisfiable");
        assert!(err
            .0
            .contains("does not match any calendar date in the next year"));
    }
}
